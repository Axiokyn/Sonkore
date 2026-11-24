// Headless Wayland compositor for testing Sonkore (Rust/winit)
// with a real window at any scale, without a display: several surfaces
// (toplevels and popups), pointer and keyboard. Each surface keeps a
// "screen" copy updated only where damaged, the last buffer whole, and the
// alpha of its first full frame (the reference: what is opaque by design).
// Built and driven by tools/ui-live.py.
// stdin commands:
//   move X Y | focus main|last|lasttop|N | button 0|1 | click | dclick | rclick |
//   axis DY | leave | dismiss | resize W H | scale N | check NAME |
//   dump NAME | list | quit |
//   key [ctrl+][shift+][alt+]NAME (a-z 0-9 space enter escape tab comma period minus up down
//       left right delete backspace home end pageup pagedown f1..f12) |
//   (a toplevel's move request is printed as "moved toplevel 1")
//   lat NAME: the time to the next buffer committed, printed as
//       "lat NAME MS" (stderr, or RESULT_FILE) | latnew NAME: the same, to
//       the first buffer of a new surface (a window or popup opening)
// env: START_SCALE (x120, default 180 = 150 %), MAXIMIZED, DELAY_RELEASE,
//      OUTPUT WxH (physical, default 2546x1420), RESULT_FILE, RESULT_TAG, DUMP_DIR
#define _GNU_SOURCE
#include <wayland-server.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <stdint.h>
#include <unistd.h>
#include <math.h>
#include <time.h>
#include <sys/mman.h>
#include <pixman.h>
#include <xkbcommon/xkbcommon.h>
#include "xdg-shell-server.h"
#include "viewporter-server.h"
#include "fractional-scale-v1-server.h"

static struct wl_display* display;
static int logical_w = 1697, logical_h = 947;
static int output_w = 2546, output_h = 1420;
static int delay_release = 0;
static struct wl_list frame_callbacks;

enum { ROLE_NONE, ROLE_TOPLEVEL, ROLE_POPUP };

struct surface {
    struct wl_resource* res;
    struct wl_resource* pending_buffer;
    int has_pending_buffer;
    pixman_region32_t dmg_surface, dmg_buffer;
    int role;
    struct wl_resource* xdg_surface;
    struct wl_resource* role_res;
    int configured;
    int dest_w, dest_h;
    int pw, ph, px, py; // popup size / position from its positioner
    int w, h;
    uint32_t* screen;
    uint32_t* lastbuf;
    uint8_t* ref;
    int index;
    struct wl_resource* held;
    struct wl_listener held_listener;
    int had_buffer;
};

#define MAXS 512
static struct surface* surfaces[MAXS];
static int surface_count = 0;
static int toplevel_count = 0;

static struct wl_resource* pointer_res = NULL;
static struct surface* focus = NULL;
static struct surface* entered = NULL;
static double px_last = 10, py_last = 10;

// Latency: "lat NAME" stamps now; the next committed buffer reports it.
static char lat_name[256];
static double lat_start = 0;
static int lat_new = 0; // 1: only a surface's first buffer counts (a new window or popup)
static double now_ms(void)
{
    struct timespec t;
    clock_gettime(CLOCK_MONOTONIC, &t);
    return t.tv_sec * 1000.0 + t.tv_nsec / 1e6;
}
static void report(const char* fmt, const char* name, double value)
{
    const char* path = getenv("RESULT_FILE");
    FILE* f = path ? fopen(path, "a") : stderr;
    if (!f) return;
    fprintf(f, fmt, name, value);
    if (path) fclose(f); else fflush(f);
}

static void noop(void) {}
static struct wl_resource* outputs[64];
static int output_count;
static void res_destroy(struct wl_client* c, struct wl_resource* r) { (void)c; wl_resource_destroy(r); }
static void* destroy_impl[32];
static uint32_t next_serial(void) { return wl_display_next_serial(display); }

static struct surface* main_surface(void)
{
    for (int i = 0; i < surface_count; ++i)
        if (surfaces[i] && surfaces[i]->role == ROLE_TOPLEVEL)
            return surfaces[i];
    return NULL;
}
static struct surface* last_surface(void)
{
    for (int i = surface_count - 1; i >= 0; --i)
        if (surfaces[i] && surfaces[i]->role != ROLE_NONE && surfaces[i]->screen)
            return surfaces[i];
    return NULL;
}

// --- frame callbacks
static int frame_tick(void* data)
{
    struct wl_resource *r, *tmp;
    wl_resource_for_each_safe(r, tmp, &frame_callbacks) {
        wl_callback_send_done(r, 0);
        wl_resource_destroy(r);
    }
    wl_event_source_timer_update(*(struct wl_event_source**)data, 16);
    return 0;
}
static void cb_destroy(struct wl_resource* r) { wl_list_remove(wl_resource_get_link(r)); }

// --- held buffers (delay release by one commit, as a compositor that keeps the last one)
static void held_gone(struct wl_listener* l, void* d)
{
    (void)d;
    struct surface* s = wl_container_of(l, s, held_listener);
    s->held = NULL;
    wl_list_remove(&l->link);
}
static void hold(struct surface* s, struct wl_resource* b)
{
    if (s->held == b) return;
    if (s->held) { wl_list_remove(&s->held_listener.link); wl_buffer_send_release(s->held); }
    s->held = b;
    s->held_listener.notify = held_gone;
    wl_resource_add_destroy_listener(b, &s->held_listener);
}

// --- region
static const struct wl_region_interface region_impl = { .destroy = res_destroy, .add = (void*)noop, .subtract = (void*)noop };

// --- surface
static void surface_resource_destroy(struct wl_resource* r)
{
    struct surface* s = wl_resource_get_user_data(r);
    if (focus == s) focus = NULL;
    if (entered == s) entered = NULL;
    if (s->held) { wl_list_remove(&s->held_listener.link); s->held = NULL; }
    surfaces[s->index] = NULL;
    pixman_region32_fini(&s->dmg_surface);
    pixman_region32_fini(&s->dmg_buffer);
    free(s->screen); free(s->lastbuf); free(s->ref);
    free(s);
}
static void surface_attach(struct wl_client* c, struct wl_resource* r, struct wl_resource* buffer, int32_t x, int32_t y)
{
    (void)c; (void)x; (void)y;
    struct surface* s = wl_resource_get_user_data(r);
    s->pending_buffer = buffer;
    s->has_pending_buffer = 1;
}
static void surface_damage(struct wl_client* c, struct wl_resource* r, int32_t x, int32_t y, int32_t w, int32_t h)
{
    (void)c;
    struct surface* s = wl_resource_get_user_data(r);
    pixman_region32_union_rect(&s->dmg_surface, &s->dmg_surface, x, y, w, h);
}
static void surface_damage_buffer(struct wl_client* c, struct wl_resource* r, int32_t x, int32_t y, int32_t w, int32_t h)
{
    (void)c;
    struct surface* s = wl_resource_get_user_data(r);
    pixman_region32_union_rect(&s->dmg_buffer, &s->dmg_buffer, x, y, w, h);
}
static void surface_frame(struct wl_client* c, struct wl_resource* r, uint32_t id)
{
    (void)r;
    struct wl_resource* cb = wl_resource_create(c, &wl_callback_interface, 1, id);
    wl_resource_set_implementation(cb, NULL, NULL, cb_destroy);
    wl_list_insert(frame_callbacks.prev, wl_resource_get_link(cb));
}
static void copy_rect(struct surface* s, const uint32_t* src, int stride_px, int x, int y, int w, int h)
{
    for (int yy = y; yy < y + h; ++yy) {
        if (yy < 0 || yy >= s->h) continue;
        for (int xx = x; xx < x + w; ++xx) {
            if (xx < 0 || xx >= s->w) continue;
            s->screen[yy * s->w + xx] = src[yy * stride_px + xx];
        }
    }
}
static void configure_role(struct surface* s)
{
    struct wl_array states;
    wl_array_init(&states);
    if (s->role == ROLE_TOPLEVEL) {
        uint32_t* st = wl_array_add(&states, sizeof(uint32_t));
        *st = XDG_TOPLEVEL_STATE_ACTIVATED;
        int w = 0, h = 0;
        if (s == main_surface() && getenv("MAXIMIZED")) {
            st = wl_array_add(&states, sizeof(uint32_t));
            *st = XDG_TOPLEVEL_STATE_MAXIMIZED;
            w = logical_w; h = logical_h;
        }
        xdg_toplevel_send_configure(s->role_res, w, h, &states);
    } else if (s->role == ROLE_POPUP) {
        xdg_popup_send_configure(s->role_res, s->px, s->py, s->pw, s->ph);
    }
    wl_array_release(&states);
    xdg_surface_send_configure(s->xdg_surface, next_serial());
}
static void surface_commit(struct wl_client* c, struct wl_resource* r)
{
    (void)c;
    struct surface* s = wl_resource_get_user_data(r);
    if (s->role != ROLE_NONE && !s->configured) {
        configure_role(s);
        s->configured = 1;
        // On the one output: winit's current_monitor() needs it (the
        // windows open within the screen's height).
        for (int i = 0; i < output_count; ++i)
            if (outputs[i] && wl_resource_get_client(outputs[i]) == wl_resource_get_client(s->res))
                wl_surface_send_enter(s->res, outputs[i]);
    }
    if (s->has_pending_buffer && s->pending_buffer) {
        struct wl_shm_buffer* shm = wl_shm_buffer_get(s->pending_buffer);
        if (shm) {
            int w = wl_shm_buffer_get_width(shm), h = wl_shm_buffer_get_height(shm);
            int stride = wl_shm_buffer_get_stride(shm) / 4;
            wl_shm_buffer_begin_access(shm);
            const uint32_t* data = wl_shm_buffer_get_data(shm);
            int full = 0;
            if (w != s->w || h != s->h) {
                free(s->screen); free(s->lastbuf); free(s->ref);
                s->w = w; s->h = h;
                s->screen = calloc((size_t)w * h, 4);
                s->lastbuf = calloc((size_t)w * h, 4);
                s->ref = calloc((size_t)w * h, 1);
                full = 1;
            }
            for (int yy = 0; yy < h; ++yy)
                memcpy(s->lastbuf + (size_t)yy * w, data + (size_t)yy * stride, (size_t)w * 4);
            if (full) {
                copy_rect(s, data, stride, 0, 0, w, h);
                for (int i = 0; i < w * h; ++i) s->ref[i] = s->screen[i] >> 24;
            } else {
                int n;
                pixman_box32_t* b = pixman_region32_rectangles(&s->dmg_buffer, &n);
                for (int i = 0; i < n; ++i)
                    copy_rect(s, data, stride, b[i].x1, b[i].y1, b[i].x2 - b[i].x1, b[i].y2 - b[i].y1);
                double sx = s->dest_w > 0 ? (double)w / s->dest_w : 1, sy = s->dest_h > 0 ? (double)h / s->dest_h : 1;
                b = pixman_region32_rectangles(&s->dmg_surface, &n);
                for (int i = 0; i < n; ++i) {
                    int x1 = (int)floor(b[i].x1 * sx), y1 = (int)floor(b[i].y1 * sy);
                    int x2 = (int)ceil(b[i].x2 * sx), y2 = (int)ceil(b[i].y2 * sy);
                    copy_rect(s, data, stride, x1, y1, x2 - x1, y2 - y1);
                }
            }
            wl_shm_buffer_end_access(shm);
        }
        if (lat_start > 0 && (!lat_new || !s->had_buffer)) {
            report("lat %s %.0f\n", lat_name, now_ms() - lat_start);
            lat_start = 0;
        }
        s->had_buffer = 1;
        if (delay_release) hold(s, s->pending_buffer);
        else wl_buffer_send_release(s->pending_buffer);
    }
    s->has_pending_buffer = 0;
    pixman_region32_clear(&s->dmg_surface);
    pixman_region32_clear(&s->dmg_buffer);
}
static const struct wl_surface_interface surface_impl = {
    .destroy = res_destroy, .attach = surface_attach, .damage = surface_damage, .frame = surface_frame,
    .set_opaque_region = (void*)noop, .set_input_region = (void*)noop, .commit = surface_commit,
    .set_buffer_transform = (void*)noop, .set_buffer_scale = (void*)noop, .damage_buffer = surface_damage_buffer,
};
static void compositor_create_surface(struct wl_client* c, struct wl_resource* r, uint32_t id)
{
    struct surface* s = calloc(1, sizeof *s);
    s->res = wl_resource_create(c, &wl_surface_interface, wl_resource_get_version(r), id);
    pixman_region32_init(&s->dmg_surface);
    pixman_region32_init(&s->dmg_buffer);
    s->index = surface_count < MAXS ? surface_count++ : MAXS - 1;
    surfaces[s->index] = s;
    wl_resource_set_implementation(s->res, &surface_impl, s, surface_resource_destroy);
}
static void compositor_create_region(struct wl_client* c, struct wl_resource* r, uint32_t id)
{
    struct wl_resource* reg = wl_resource_create(c, &wl_region_interface, wl_resource_get_version(r), id);
    wl_resource_set_implementation(reg, &region_impl, NULL, NULL);
}
static const struct wl_compositor_interface compositor_impl = { .create_surface = compositor_create_surface, .create_region = compositor_create_region };
static void bind_compositor(struct wl_client* c, void* d, uint32_t v, uint32_t id)
{
    (void)d;
    struct wl_resource* r = wl_resource_create(c, &wl_compositor_interface, v, id);
    wl_resource_set_implementation(r, &compositor_impl, NULL, NULL);
}

// --- xdg shell
struct positioner { int w, h, ax, ay, aw, ah, ox, oy, anchor; };
static void pos_destroy_res(struct wl_resource* r) { free(wl_resource_get_user_data(r)); }
static void pos_set_size(struct wl_client* c, struct wl_resource* r, int32_t w, int32_t h) { (void)c; struct positioner* p = wl_resource_get_user_data(r); p->w = w; p->h = h; }
static void pos_set_anchor_rect(struct wl_client* c, struct wl_resource* r, int32_t x, int32_t y, int32_t w, int32_t h)
{ (void)c; struct positioner* p = wl_resource_get_user_data(r); p->ax = x; p->ay = y; p->aw = w; p->ah = h; }
static void pos_set_anchor(struct wl_client* c, struct wl_resource* r, uint32_t a) { (void)c; struct positioner* p = wl_resource_get_user_data(r); p->anchor = (int)a; }
static void pos_set_offset(struct wl_client* c, struct wl_resource* r, int32_t x, int32_t y) { (void)c; struct positioner* p = wl_resource_get_user_data(r); p->ox = x; p->oy = y; }
static const struct xdg_positioner_interface positioner_impl = {
    .destroy = res_destroy, .set_size = pos_set_size, .set_anchor_rect = pos_set_anchor_rect, .set_anchor = pos_set_anchor,
    .set_gravity = (void*)noop, .set_constraint_adjustment = (void*)noop, .set_offset = pos_set_offset,
    .set_reactive = (void*)noop, .set_parent_size = (void*)noop, .set_parent_configure = (void*)noop };
static void positioner_create(struct wl_client* c, struct wl_resource* r, uint32_t id)
{
    struct wl_resource* p = wl_resource_create(c, &xdg_positioner_interface, wl_resource_get_version(r), id);
    wl_resource_set_implementation(p, &positioner_impl, calloc(1, sizeof(struct positioner)), pos_destroy_res);
}
static void role_destroyed(struct wl_resource* r)
{
    struct surface* s = wl_resource_get_user_data(r);
    if (s) { s->role = ROLE_NONE; s->role_res = NULL; s->configured = 0; }
}
static void popup_grab(void) {}
// xdg_toplevel.move: the window asks to be moved with the pointer (a title
// bar press). Printed as "moved toplevel 1" so a scenario can check it.
static void toplevel_move(struct wl_client* c, struct wl_resource* r, struct wl_resource* seat, uint32_t serial)
{
    (void)c; (void)r; (void)seat; (void)serial;
    report("moved %s %g\n", "toplevel", 1);
}
static const struct xdg_popup_interface popup_impl = { .destroy = res_destroy, .grab = (void*)popup_grab, .reposition = (void*)noop };
static const struct xdg_toplevel_interface toplevel_impl = {
    .destroy = res_destroy, .set_parent = (void*)noop, .set_title = (void*)noop, .set_app_id = (void*)noop,
    .show_window_menu = (void*)noop, .move = toplevel_move, .resize = (void*)noop, .set_max_size = (void*)noop,
    .set_min_size = (void*)noop, .set_maximized = (void*)noop, .unset_maximized = (void*)noop,
    .set_fullscreen = (void*)noop, .unset_fullscreen = (void*)noop, .set_minimized = (void*)noop };
static void xdg_surface_get_toplevel(struct wl_client* c, struct wl_resource* r, uint32_t id)
{
    struct surface* s = wl_resource_get_user_data(r);
    struct wl_resource* t = wl_resource_create(c, &xdg_toplevel_interface, wl_resource_get_version(r), id);
    wl_resource_set_implementation(t, &toplevel_impl, s, role_destroyed);
    s->role = ROLE_TOPLEVEL;
    s->role_res = t;
    s->configured = 0;
    ++toplevel_count;
}
static void xdg_surface_get_popup(struct wl_client* c, struct wl_resource* r, uint32_t id, struct wl_resource* parent, struct wl_resource* positioner)
{
    (void)parent;
    struct surface* s = wl_resource_get_user_data(r);
    struct positioner* p = wl_resource_get_user_data(positioner);
    struct wl_resource* pr = wl_resource_create(c, &xdg_popup_interface, wl_resource_get_version(r), id);
    wl_resource_set_implementation(pr, &popup_impl, s, role_destroyed);
    s->role = ROLE_POPUP;
    s->role_res = pr;
    s->configured = 0;
    s->pw = p->w > 0 ? p->w : 100; s->ph = p->h > 0 ? p->h : 100;
    s->px = p->ax + p->ox; s->py = p->ay + p->ah + p->oy;
}
static void xdg_surface_destroy_res(struct wl_resource* r)
{
    struct surface* s = wl_resource_get_user_data(r);
    if (s && s->xdg_surface == r) s->xdg_surface = NULL;
}
static const struct xdg_surface_interface xdg_surface_impl = {
    .destroy = res_destroy, .get_toplevel = xdg_surface_get_toplevel, .get_popup = xdg_surface_get_popup,
    .set_window_geometry = (void*)noop, .ack_configure = (void*)noop };
static void wm_get_xdg_surface(struct wl_client* c, struct wl_resource* r, uint32_t id, struct wl_resource* surface)
{
    struct surface* s = wl_resource_get_user_data(surface);
    struct wl_resource* x = wl_resource_create(c, &xdg_surface_interface, wl_resource_get_version(r), id);
    wl_resource_set_implementation(x, &xdg_surface_impl, s, xdg_surface_destroy_res);
    s->xdg_surface = x;
}
static const struct xdg_wm_base_interface wm_impl = {
    .destroy = res_destroy, .create_positioner = positioner_create, .get_xdg_surface = wm_get_xdg_surface, .pong = (void*)noop };
static void bind_wm(struct wl_client* c, void* d, uint32_t v, uint32_t id)
{
    (void)d;
    struct wl_resource* r = wl_resource_create(c, &xdg_wm_base_interface, v, id);
    wl_resource_set_implementation(r, &wm_impl, NULL, NULL);
}

// --- viewporter
static void viewport_set_destination(struct wl_client* c, struct wl_resource* r, int32_t w, int32_t h)
{
    (void)c;
    struct surface* s = wl_resource_get_user_data(r);
    if (s) { s->dest_w = w; s->dest_h = h; }
}
static const struct wp_viewport_interface viewport_impl = { .destroy = res_destroy, .set_source = (void*)noop, .set_destination = viewport_set_destination };
static void viewporter_get(struct wl_client* c, struct wl_resource* r, uint32_t id, struct wl_resource* surface)
{
    struct wl_resource* v = wl_resource_create(c, &wp_viewport_interface, wl_resource_get_version(r), id);
    wl_resource_set_implementation(v, &viewport_impl, wl_resource_get_user_data(surface), NULL);
}
static const struct wp_viewporter_interface viewporter_impl = { .destroy = res_destroy, .get_viewport = viewporter_get };
static void bind_viewporter(struct wl_client* c, void* d, uint32_t v, uint32_t id)
{
    (void)d;
    struct wl_resource* r = wl_resource_create(c, &wp_viewporter_interface, v, id);
    wl_resource_set_implementation(r, &viewporter_impl, NULL, NULL);
}

// --- fractional scale
static struct wl_resource* fs_objects[MAXS];
static int fs_count = 0;
static int pref_scale = 180;
static void fs_destroyed(struct wl_resource* r)
{
    for (int i = 0; i < fs_count; ++i)
        if (fs_objects[i] == r) fs_objects[i] = NULL;
}
static void fs_get(struct wl_client* c, struct wl_resource* r, uint32_t id, struct wl_resource* surface)
{
    (void)surface;
    struct wl_resource* f = wl_resource_create(c, &wp_fractional_scale_v1_interface, wl_resource_get_version(r), id);
    wl_resource_set_implementation(f, destroy_impl, NULL, fs_destroyed);
    if (fs_count < MAXS) fs_objects[fs_count++] = f;
    wp_fractional_scale_v1_send_preferred_scale(f, pref_scale);
}
static const struct wp_fractional_scale_manager_v1_interface fs_impl = { .destroy = res_destroy, .get_fractional_scale = fs_get };
static void bind_fs(struct wl_client* c, void* d, uint32_t v, uint32_t id)
{
    (void)d;
    struct wl_resource* r = wl_resource_create(c, &wp_fractional_scale_manager_v1_interface, v, id);
    wl_resource_set_implementation(r, &fs_impl, NULL, NULL);
}

// --- subcompositor (sctk binds it; subsurfaces are not drawn here)
static const struct wl_subsurface_interface subsurface_impl = {
    .destroy = res_destroy, .set_position = (void*)noop, .place_above = (void*)noop, .place_below = (void*)noop,
    .set_sync = (void*)noop, .set_desync = (void*)noop };
static void subcompositor_get(struct wl_client* c, struct wl_resource* r, uint32_t id, struct wl_resource* surface, struct wl_resource* parent)
{
    (void)surface; (void)parent;
    struct wl_resource* s = wl_resource_create(c, &wl_subsurface_interface, wl_resource_get_version(r), id);
    wl_resource_set_implementation(s, &subsurface_impl, NULL, NULL);
}
static const struct wl_subcompositor_interface subcompositor_impl = { .destroy = res_destroy, .get_subsurface = subcompositor_get };
static void bind_subcompositor(struct wl_client* c, void* d, uint32_t v, uint32_t id)
{
    (void)d;
    struct wl_resource* r = wl_resource_create(c, &wl_subcompositor_interface, v, id);
    wl_resource_set_implementation(r, &subcompositor_impl, NULL, NULL);
}

// --- output
static void output_gone(struct wl_resource* r)
{
    for (int i = 0; i < output_count; ++i)
        if (outputs[i] == r) outputs[i] = NULL;
}
static void bind_output(struct wl_client* c, void* d, uint32_t v, uint32_t id)
{
    (void)d;
    struct wl_resource* r = wl_resource_create(c, &wl_output_interface, v, id);
    wl_resource_set_implementation(r, destroy_impl, NULL, output_gone);
    if (output_count < 64) outputs[output_count++] = r;
    wl_output_send_geometry(r, 0, 0, 600, 340, WL_OUTPUT_SUBPIXEL_UNKNOWN, "Test", "Headless", WL_OUTPUT_TRANSFORM_NORMAL);
    wl_output_send_mode(r, WL_OUTPUT_MODE_CURRENT, output_w, output_h, 60000);
    if (v >= 2) { wl_output_send_scale(r, 2); wl_output_send_done(r); }
}

// --- seat / pointer
static void pointer_destroy(struct wl_resource* r) { if (r == pointer_res) pointer_res = NULL; }
static void pointer_release(struct wl_client* c, struct wl_resource* r) { res_destroy(c, r); }
static const struct wl_pointer_interface pointer_impl = { .set_cursor = (void*)noop, .release = pointer_release };
static void seat_get_pointer(struct wl_client* c, struct wl_resource* r, uint32_t id)
{
    struct wl_resource* p = wl_resource_create(c, &wl_pointer_interface, wl_resource_get_version(r), id);
    wl_resource_set_implementation(p, &pointer_impl, NULL, pointer_destroy);
    pointer_res = p;
}
// --- keyboard (a US keymap from xkbcommon)
static struct wl_resource* keyboard_res = NULL;
static struct surface* kb_entered = NULL;
static struct xkb_keymap* keymap = NULL;
static char* keymap_text = NULL;
static void keyboard_destroy(struct wl_resource* r) { if (r == keyboard_res) keyboard_res = NULL; }
static const struct wl_keyboard_interface keyboard_impl = { .release = res_destroy };
static void seat_get_keyboard(struct wl_client* c, struct wl_resource* r, uint32_t id)
{
    struct wl_resource* k = wl_resource_create(c, &wl_keyboard_interface, wl_resource_get_version(r), id);
    wl_resource_set_implementation(k, &keyboard_impl, NULL, keyboard_destroy);
    keyboard_res = k;
    kb_entered = NULL;
    size_t size = strlen(keymap_text) + 1;
    int fd = memfd_create("keymap", MFD_CLOEXEC);
    if (fd >= 0 && write(fd, keymap_text, size) == (ssize_t)size) {
        wl_keyboard_send_keymap(k, WL_KEYBOARD_KEYMAP_FORMAT_XKB_V1, fd, (uint32_t)size);
    }
    if (fd >= 0) close(fd);
    if (wl_resource_get_version(k) >= 4) wl_keyboard_send_repeat_info(k, 0, 0);
}
static const struct wl_seat_interface seat_impl = { .get_pointer = seat_get_pointer, .get_keyboard = seat_get_keyboard, .get_touch = (void*)noop, .release = res_destroy };
static void bind_seat(struct wl_client* c, void* d, uint32_t v, uint32_t id)
{
    (void)d;
    struct wl_resource* r = wl_resource_create(c, &wl_seat_interface, v, id);
    wl_resource_set_implementation(r, &seat_impl, NULL, NULL);
    wl_seat_send_capabilities(r, WL_SEAT_CAPABILITY_POINTER | WL_SEAT_CAPABILITY_KEYBOARD);
    if (v >= 2) wl_seat_send_name(r, "seat0");
}
static const struct { const char* name; int code; } key_codes[] = {
    {"escape", 1}, {"1", 2}, {"2", 3}, {"3", 4}, {"4", 5}, {"5", 6}, {"6", 7}, {"7", 8}, {"8", 9}, {"9", 10}, {"0", 11},
    {"backspace", 14}, {"tab", 15}, {"q", 16}, {"w", 17}, {"e", 18}, {"r", 19}, {"t", 20}, {"y", 21}, {"u", 22},
    {"i", 23}, {"o", 24}, {"p", 25}, {"enter", 28}, {"a", 30}, {"s", 31}, {"d", 32}, {"f", 33}, {"g", 34}, {"h", 35},
    {"j", 36}, {"k", 37}, {"l", 38}, {"z", 44}, {"x", 45}, {"c", 46}, {"v", 47}, {"b", 48}, {"n", 49}, {"m", 50},
    {"space", 57}, {"f1", 59}, {"f2", 60}, {"f3", 61}, {"f4", 62}, {"f5", 63}, {"f6", 64}, {"f7", 65}, {"f8", 66},
    {"f9", 67}, {"f10", 68}, {"f11", 87}, {"f12", 88}, {"home", 102}, {"up", 103}, {"pageup", 104}, {"left", 105},
    {"right", 106}, {"end", 107}, {"down", 108}, {"pagedown", 109}, {"delete", 111},
    {"comma", 51}, {"period", 52}, {"minus", 12},
};
static struct surface* target(void);
static void key_enter_if_needed(void)
{
    struct surface* t = target();
    if (!keyboard_res || !t || kb_entered == t) return;
    if (kb_entered) wl_keyboard_send_leave(keyboard_res, next_serial(), kb_entered->res);
    struct wl_array keys;
    wl_array_init(&keys);
    wl_keyboard_send_enter(keyboard_res, next_serial(), t->res, &keys);
    wl_array_release(&keys);
    wl_keyboard_send_modifiers(keyboard_res, next_serial(), 0, 0, 0, 0);
    kb_entered = t;
}
static void send_key(const char* spec)
{
    uint32_t mods = 0;
    const char* name = spec;
    for (;;) {
        if (!strncmp(name, "ctrl+", 5)) { mods |= 1u << xkb_keymap_mod_get_index(keymap, XKB_MOD_NAME_CTRL); name += 5; }
        else if (!strncmp(name, "shift+", 6)) { mods |= 1u << xkb_keymap_mod_get_index(keymap, XKB_MOD_NAME_SHIFT); name += 6; }
        else if (!strncmp(name, "alt+", 4)) { mods |= 1u << xkb_keymap_mod_get_index(keymap, XKB_MOD_NAME_ALT); name += 4; }
        else break;
    }
    int code = -1;
    for (size_t i = 0; i < sizeof key_codes / sizeof key_codes[0]; ++i)
        if (!strcmp(key_codes[i].name, name)) code = key_codes[i].code;
    if (code < 0) { fprintf(stderr, "unknown key %s\n", name); return; }
    key_enter_if_needed();
    if (!keyboard_res) { fprintf(stderr, "no keyboard bound\n"); return; }
    // As a real compositor: the modifier keys themselves, then the state.
    static const struct { const char* mod; int code; } mod_keys[] = {
        {XKB_MOD_NAME_CTRL, 29}, {XKB_MOD_NAME_SHIFT, 42}, {XKB_MOD_NAME_ALT, 56} };
    uint32_t t = (uint32_t)now_ms();
    for (int i = 0; i < 3; ++i)
        if (mods & (1u << xkb_keymap_mod_get_index(keymap, mod_keys[i].mod)))
            wl_keyboard_send_key(keyboard_res, next_serial(), t, (uint32_t)mod_keys[i].code, WL_KEYBOARD_KEY_STATE_PRESSED);
    if (mods) wl_keyboard_send_modifiers(keyboard_res, next_serial(), mods, 0, 0, 0);
    wl_keyboard_send_key(keyboard_res, next_serial(), t, (uint32_t)code, WL_KEYBOARD_KEY_STATE_PRESSED);
    wl_keyboard_send_key(keyboard_res, next_serial(), t + 30, (uint32_t)code, WL_KEYBOARD_KEY_STATE_RELEASED);
    for (int i = 0; i < 3; ++i)
        if (mods & (1u << xkb_keymap_mod_get_index(keymap, mod_keys[i].mod)))
            wl_keyboard_send_key(keyboard_res, next_serial(), t + 40, (uint32_t)mod_keys[i].code, WL_KEYBOARD_KEY_STATE_RELEASED);
    if (mods) wl_keyboard_send_modifiers(keyboard_res, next_serial(), 0, 0, 0, 0);
}

// --- commands
static void save_pam(const char* path, const uint32_t* px, int w, int h)
{
    FILE* f = fopen(path, "wb");
    if (!f) return;
    fprintf(f, "P7\nWIDTH %d\nHEIGHT %d\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n", w, h);
    for (int i = 0; i < w * h; ++i) {
        uint32_t p = px[i];
        unsigned char q[4] = { (p >> 16) & 255, (p >> 8) & 255, p & 255, (p >> 24) & 255 };
        fwrite(q, 1, 4, f);
    }
    fclose(f);
}
static const char* role_name(int role) { return role == ROLE_TOPLEVEL ? "toplevel" : role == ROLE_POPUP ? "popup" : "none"; }
static struct surface* target(void)
{
    if (focus && surfaces[focus->index] == focus) return focus;
    focus = main_surface();
    return focus;
}
static void enter_if_needed(void)
{
    struct surface* t = target();
    if (!pointer_res || !t) return;
    if (entered != t) {
        if (entered) wl_pointer_send_leave(pointer_res, next_serial(), entered->res);
        wl_pointer_send_enter(pointer_res, next_serial(), t->res, wl_fixed_from_double(px_last), wl_fixed_from_double(py_last));
        entered = t;
    }
}
static void frame_ev(void) { if (pointer_res && wl_resource_get_version(pointer_res) >= 5) wl_pointer_send_frame(pointer_res); }
static void press(uint32_t button)
{
    enter_if_needed();
    if (!pointer_res) return;
    // Real times, always forward: toolkits tell a double click by them.
    static uint32_t last;
    uint32_t t = (uint32_t)now_ms();
    if (t < last + 40) t = last + 40;
    last = t;
    wl_pointer_send_button(pointer_res, next_serial(), t, button, 1); frame_ev();
    wl_pointer_send_button(pointer_res, next_serial(), t + 20, button, 0); frame_ev();
}
static char linebuf[8192];
static size_t linelen = 0;
static void handle_line(char* line)
{
    double x, y;
    int a, b;
    char name[1024];
    if (sscanf(line, "move %lf %lf", &x, &y) == 2) {
        px_last = x; py_last = y;
        enter_if_needed();
        if (pointer_res && entered) { wl_pointer_send_motion(pointer_res, 0, wl_fixed_from_double(x), wl_fixed_from_double(y)); frame_ev(); }
    } else if (sscanf(line, "focus %1000s", name) == 1) {
        if (!strcmp(name, "main")) focus = main_surface();
        else if (!strcmp(name, "last")) focus = last_surface();
        else if (!strcmp(name, "lasttop")) {
            for (int i = surface_count - 1; i >= 0; --i)
                if (surfaces[i] && surfaces[i]->role == ROLE_TOPLEVEL && surfaces[i]->screen) { focus = surfaces[i]; break; }
        }
        else { int i = atoi(name); focus = i >= 0 && i < MAXS ? surfaces[i] : NULL; }
    } else if (strncmp(line, "leave", 5) == 0) {
        if (pointer_res && entered) { wl_pointer_send_leave(pointer_res, next_serial(), entered->res); frame_ev(); entered = NULL; }
    } else if (sscanf(line, "button %d", &a) == 1) {
        enter_if_needed();
        if (pointer_res) { wl_pointer_send_button(pointer_res, next_serial(), (uint32_t)now_ms(), 0x110, a ? 1 : 0); frame_ev(); }
    } else if (strncmp(line, "dclick", 6) == 0) {
        press(0x110); press(0x110);
    } else if (strncmp(line, "rclick", 6) == 0) {
        press(0x111);
    } else if (strncmp(line, "click", 5) == 0) {
        press(0x110);
    } else if (sscanf(line, "axis %lf", &y) == 1) {
        enter_if_needed();
        if (!pointer_res) return;
        if (wl_resource_get_version(pointer_res) >= 5) {
            wl_pointer_send_axis_source(pointer_res, WL_POINTER_AXIS_SOURCE_WHEEL);
            wl_pointer_send_axis_discrete(pointer_res, 0, y > 0 ? 1 : -1);
        }
        wl_pointer_send_axis(pointer_res, 0, 0, wl_fixed_from_double(y));
        frame_ev();
    } else if (strncmp(line, "dismiss", 7) == 0) {
        for (int i = surface_count - 1; i >= 0; --i)
            if (surfaces[i] && surfaces[i]->role == ROLE_POPUP && surfaces[i]->role_res)
                xdg_popup_send_popup_done(surfaces[i]->role_res);
    } else if (sscanf(line, "resize %d %d", &a, &b) == 2) {
        logical_w = a; logical_h = b;
        struct surface* m = main_surface();
        if (m && m->role_res) {
            struct wl_array states;
            wl_array_init(&states);
            uint32_t* st = wl_array_add(&states, sizeof(uint32_t));
            *st = XDG_TOPLEVEL_STATE_ACTIVATED;
            xdg_toplevel_send_configure(m->role_res, a, b, &states);
            wl_array_release(&states);
            xdg_surface_send_configure(m->xdg_surface, next_serial());
        }
    } else if (sscanf(line, "scale %d", &a) == 1) {
        pref_scale = a;
        for (int i = 0; i < fs_count; ++i)
            if (fs_objects[i]) wp_fractional_scale_v1_send_preferred_scale(fs_objects[i], a);
    } else if (sscanf(line, "check %1000s", name) == 1) {
        const char* tag = getenv("RESULT_TAG");
        FILE* f = fopen(getenv("RESULT_FILE") ? getenv("RESULT_FILE") : "/dev/stderr", "a");
        for (int i = 0; i < surface_count; ++i) {
            struct surface* s = surfaces[i];
            if (!s || s->role == ROLE_NONE || !s->screen) continue;
            long holes = 0, miss = 0;
            for (int k = 0; k < s->w * s->h; ++k) {
                if (s->screen[k] != s->lastbuf[k]) ++miss;
                if (s->ref[k] >= 250 && (s->screen[k] >> 24) < 250) ++holes;
            }
            if (f) fprintf(f, "%s %s#%d %s %d %d %ld %ld\n", tag ? tag : "", name, i, role_name(s->role), s->w, s->h, holes, miss);
            if ((holes || miss) && getenv("DUMP_DIR")) {
                char p[1200];
                snprintf(p, sizeof p, "%s/%s_%d_screen.pam", getenv("DUMP_DIR"), name, i);
                save_pam(p, s->screen, s->w, s->h);
                snprintf(p, sizeof p, "%s/%s_%d_buffer.pam", getenv("DUMP_DIR"), name, i);
                save_pam(p, s->lastbuf, s->w, s->h);
            }
        }
        if (f) fclose(f);
    } else if (sscanf(line, "dump %1000s", name) == 1) {
        for (int i = 0; i < surface_count; ++i) {
            struct surface* s = surfaces[i];
            if (!s || s->role == ROLE_NONE || !s->screen) continue;
            char p[1200];
            snprintf(p, sizeof p, "%s_%d_screen.pam", name, i);
            save_pam(p, s->screen, s->w, s->h);
        }
    } else if (strncmp(line, "list", 4) == 0) {
        for (int i = 0; i < surface_count; ++i) {
            struct surface* s = surfaces[i];
            if (s && s->role != ROLE_NONE) fprintf(stderr, "surface %d %s %dx%d%s\n", i, role_name(s->role), s->w, s->h, s == target() ? " (focus)" : "");
        }
        fprintf(stderr, "list-end\n");
    } else if (sscanf(line, "key %1000s", name) == 1) {
        send_key(name);
    } else if (sscanf(line, "latnew %255s", lat_name) == 1) {
        lat_start = now_ms();
        lat_new = 1;
    } else if (sscanf(line, "lat %255s", lat_name) == 1) {
        lat_start = now_ms();
        lat_new = 0;
    } else if (strncmp(line, "quit", 4) == 0) {
        wl_display_terminate(display);
    }
    wl_display_flush_clients(display);
}
static int stdin_ready(int fd, uint32_t mask, void* data)
{
    (void)mask; (void)data;
    ssize_t n = read(fd, linebuf + linelen, sizeof linebuf - linelen - 1);
    if (n <= 0) { wl_display_terminate(display); return 0; }
    linelen += (size_t)n;
    linebuf[linelen] = 0;
    char* start = linebuf;
    char* nl;
    while ((nl = strchr(start, '\n'))) { *nl = 0; handle_line(start); start = nl + 1; }
    linelen = strlen(start);
    memmove(linebuf, start, linelen + 1);
    return 0;
}

int main(int argc, char** argv)
{
    const char* socket = argc > 1 ? argv[1] : "wl-test-0";
    delay_release = getenv("DELAY_RELEASE") != NULL;
    if (getenv("START_SCALE")) pref_scale = atoi(getenv("START_SCALE"));
    struct xkb_context* xkb = xkb_context_new(XKB_CONTEXT_NO_FLAGS);
    struct xkb_rule_names names = { .rules = "evdev", .model = "pc105", .layout = "us" };
    keymap = xkb ? xkb_keymap_new_from_names(xkb, &names, XKB_KEYMAP_COMPILE_NO_FLAGS) : NULL;
    if (!keymap) { fprintf(stderr, "no xkb keymap\n"); return 1; }
    keymap_text = xkb_keymap_get_as_string(keymap, XKB_KEYMAP_FORMAT_TEXT_V1);
    if (getenv("OUTPUT")) sscanf(getenv("OUTPUT"), "%dx%d", &output_w, &output_h);
    for (int i = 0; i < 32; ++i) destroy_impl[i] = (void*)noop;
    destroy_impl[0] = (void*)res_destroy;
    wl_list_init(&frame_callbacks);
    display = wl_display_create();
    if (wl_display_add_socket(display, socket) != 0) { fprintf(stderr, "cannot add socket %s\n", socket); return 1; }
    wl_display_init_shm(display);
    wl_global_create(display, &wl_compositor_interface, 4, NULL, bind_compositor);
    wl_global_create(display, &xdg_wm_base_interface, 2, NULL, bind_wm);
    wl_global_create(display, &wp_viewporter_interface, 1, NULL, bind_viewporter);
    wl_global_create(display, &wp_fractional_scale_manager_v1_interface, 1, NULL, bind_fs);
    wl_global_create(display, &wl_output_interface, 2, NULL, bind_output);
    wl_global_create(display, &wl_subcompositor_interface, 1, NULL, bind_subcompositor);
    wl_global_create(display, &wl_seat_interface, 5, NULL, bind_seat);
    struct wl_event_loop* loop = wl_display_get_event_loop(display);
    static struct wl_event_source* tick;
    tick = wl_event_loop_add_timer(loop, frame_tick, &tick);
    wl_event_source_timer_update(tick, 16);
    wl_event_loop_add_fd(loop, 0, WL_EVENT_READABLE, stdin_ready, NULL);
    fprintf(stderr, "headless compositor on %s\n", socket);
    wl_display_run(display);
    wl_display_destroy(display);
    return 0;
}
