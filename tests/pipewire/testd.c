/* A tiny PipeWire daemon for tests (run.sh): full access for every client
 * (module-access's job); the sink's and the streams' ports set up in DSP mode
 * and stream ports linked to test-sink's by channel (a session manager's job).
 * Writing another sink's node.name to $XDG_RUNTIME_DIR/test-link-target moves
 * the streams there (as Easy Effects moves them to its own sink); removing
 * the file moves them back. Config from argv[1]. */
#include <pipewire/pipewire.h>
#include <pipewire/impl.h>
#include <spa/utils/string.h>
#include <spa/param/audio/format-utils.h>
#include <spa/param/param.h>
#include <spa/pod/builder.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>

static struct pw_context *ctx;
static struct pw_loop *loop;
static struct spa_source *timer;
static char linked_to[256] = "test-sink";

/* The sink the streams go to: the file's name, else test-sink. */
static void link_target(char *out, size_t size) {
    snprintf(out, size, "test-sink");
    const char *dir = getenv("XDG_RUNTIME_DIR");
    if (!dir) return;
    char path[512];
    snprintf(path, sizeof(path), "%s/test-link-target", dir);
    FILE *f = fopen(path, "r");
    if (!f) return;
    char name[256] = "";
    if (fgets(name, sizeof(name), f)) {
        name[strcspn(name, " \r\n")] = 0;
        if (name[0]) snprintf(out, size, "%s", name);
    }
    fclose(f);
}

static void check_access(void *data, struct pw_impl_client *client) {
    struct pw_permission p = PW_PERMISSION_INIT(PW_ID_ANY, PW_PERM_ALL);
    struct spa_dict_item item = SPA_DICT_ITEM_INIT(PW_KEY_ACCESS, "unrestricted");
    struct spa_dict d = SPA_DICT_INIT(&item, 1);
    pw_impl_client_update_properties(client, &d);
    pw_impl_client_update_permissions(client, 1, &p);
    pw_impl_client_set_busy(client, false);
}

struct find { const char *channel; struct pw_impl_port *found; };

static int find_sink_port(void *data, struct pw_global *g) {
    struct find *f = data;
    if (!pw_global_is_type(g, PW_TYPE_INTERFACE_Port)) return 0;
    struct pw_impl_port *port = pw_global_get_object(g);
    if (pw_impl_port_get_direction(port) != PW_DIRECTION_INPUT) return 0;
    const struct pw_properties *np = pw_impl_node_get_properties(pw_impl_port_get_node(port));
    if (!spa_streq(pw_properties_get(np, PW_KEY_NODE_NAME), linked_to)) return 0;
    const char *ch = pw_properties_get(pw_impl_port_get_properties(port), PW_KEY_AUDIO_CHANNEL);
    if (ch && spa_streq(ch, f->channel)) { f->found = port; return 1; }
    return 0;
}

static int link_stream_port(void *data, struct pw_global *g) {
    if (!pw_global_is_type(g, PW_TYPE_INTERFACE_Port)) return 0;
    struct pw_impl_port *port = pw_global_get_object(g);
    if (pw_impl_port_get_direction(port) != PW_DIRECTION_OUTPUT) return 0;
    const struct pw_properties *np = pw_impl_node_get_properties(pw_impl_port_get_node(port));
    const char *cls = pw_properties_get(np, PW_KEY_MEDIA_CLASS);
    if (!cls || !spa_streq(cls, "Stream/Output/Audio")) return 0;
    const struct pw_properties *pp = pw_impl_port_get_properties(port);
    if (pw_properties_get(pp, "test.linked")) return 0;
    const char *ch = pw_properties_get(pp, PW_KEY_AUDIO_CHANNEL);
    if (!ch) return 0;
    struct find f = { ch, NULL };
    pw_context_for_each_global(ctx, find_sink_port, &f);
    if (!f.found) return 0;
    char *err = NULL;
    struct pw_impl_link *link = pw_context_create_link(ctx, port, f.found, NULL, NULL, 0);
    if (!link) { fprintf(stderr, "link failed\n"); return 0; }
    pw_impl_link_register(link, NULL);
    struct spa_dict_item item = SPA_DICT_ITEM_INIT("test.linked", "1");
    struct spa_dict d = SPA_DICT_INIT(&item, 1);
    pw_impl_port_update_properties(port, &d);
    fprintf(stderr, "linked %s\n", ch);
    (void)err;
    return 0;
}

static void port_config(struct pw_impl_node *node, enum spa_direction dir) {
    uint8_t buf[1024];
    struct spa_pod_builder b = SPA_POD_BUILDER_INIT(buf, sizeof(buf));
    struct spa_audio_info_raw info = { .format = SPA_AUDIO_FORMAT_F32P, .rate = 48000, .channels = 2,
        .position = { SPA_AUDIO_CHANNEL_FL, SPA_AUDIO_CHANNEL_FR } };
    struct spa_pod_frame f;
    spa_pod_builder_push_object(&b, &f, SPA_TYPE_OBJECT_ParamPortConfig, SPA_PARAM_PortConfig);
    spa_pod_builder_add(&b, SPA_PARAM_PORT_CONFIG_direction, SPA_POD_Id(dir),
        SPA_PARAM_PORT_CONFIG_mode, SPA_POD_Id(SPA_PARAM_PORT_CONFIG_MODE_dsp), 0);
    spa_pod_builder_prop(&b, SPA_PARAM_PORT_CONFIG_format, 0);
    spa_format_audio_raw_build(&b, SPA_PARAM_Format, &info);
    struct spa_pod *param = spa_pod_builder_pop(&b, &f);
    int r = spa_node_set_param(pw_impl_node_get_implementation(node), SPA_PARAM_PortConfig, 0, param);
    fprintf(stderr, "port config %s: %d\n", pw_properties_get(pw_impl_node_get_properties(node), PW_KEY_NODE_NAME), r);
}

static int configure_node(void *data, struct pw_global *g) {
    if (!pw_global_is_type(g, PW_TYPE_INTERFACE_Node)) return 0;
    struct pw_impl_node *node = pw_global_get_object(g);
    const struct pw_properties *np = pw_impl_node_get_properties(node);
    if (pw_properties_get(np, "test.configured")) return 0;
    const char *cls = pw_properties_get(np, PW_KEY_MEDIA_CLASS);
    if (!cls) return 0;
    if (spa_streq(cls, "Audio/Sink")) port_config(node, SPA_DIRECTION_INPUT);
    else if (spa_streq(cls, "Stream/Output/Audio")) port_config(node, SPA_DIRECTION_OUTPUT);
    else return 0;
    struct spa_dict_item item = SPA_DICT_ITEM_INIT("test.configured", "1");
    struct spa_dict d = SPA_DICT_INIT(&item, 1);
    pw_impl_node_update_properties(node, &d);
    return 0;
}

/* The links from the streams' ports, to be undone (collected first: a
 * link is not destroyed while the globals are walked). */
struct stream_links { struct pw_impl_link *links[64]; int n; };

static int find_stream_link(void *data, struct pw_global *g) {
    struct stream_links *l = data;
    if (!pw_global_is_type(g, PW_TYPE_INTERFACE_Link) || l->n == 64) return 0;
    struct pw_impl_link *link = pw_global_get_object(g);
    struct pw_impl_port *port = pw_impl_link_get_output(link);
    const char *cls = pw_properties_get(pw_impl_node_get_properties(pw_impl_port_get_node(port)), PW_KEY_MEDIA_CLASS);
    if (cls && spa_streq(cls, "Stream/Output/Audio")) l->links[l->n++] = link;
    return 0;
}

static void on_timer(void *data, uint64_t expirations) {
    pw_context_for_each_global(ctx, configure_node, NULL);
    char target[256];
    link_target(target, sizeof(target));
    if (!spa_streq(target, linked_to)) {
        struct stream_links l = { .n = 0 };
        pw_context_for_each_global(ctx, find_stream_link, &l);
        for (int i = 0; i < l.n; i++) {
            struct pw_impl_port *port = pw_impl_link_get_output(l.links[i]);
            struct spa_dict_item item = SPA_DICT_ITEM_INIT("test.linked", NULL);
            struct spa_dict d = SPA_DICT_INIT(&item, 1);
            pw_impl_port_update_properties(port, &d);
            pw_impl_link_destroy(l.links[i]);
        }
        snprintf(linked_to, sizeof(linked_to), "%s", target);
        fprintf(stderr, "streams to %s\n", linked_to);
    }
    pw_context_for_each_global(ctx, link_stream_port, NULL);
}

static const struct pw_context_events events = {
    PW_VERSION_CONTEXT_EVENTS,
    .check_access = check_access,
};

int main(int argc, char **argv) {
    pw_init(&argc, &argv);
    struct pw_main_loop *l = pw_main_loop_new(NULL);
    loop = pw_main_loop_get_loop(l);
    struct pw_properties *p = pw_properties_new(PW_KEY_CONFIG_NAME, argv[1], NULL);
    ctx = pw_context_new(loop, p, 0);
    if (!ctx) { perror("context"); return 1; }
    struct spa_hook hook;
    pw_context_add_listener(ctx, &hook, &events, NULL);
    timer = pw_loop_add_timer(loop, on_timer, NULL);
    struct timespec v = { 0, 50 * 1000000 }, i = { 0, 50 * 1000000 };
    pw_loop_update_timer(loop, timer, &v, &i, false);
    fprintf(stderr, "daemon up\n");
    pw_main_loop_run(l);
    return 0;
}
