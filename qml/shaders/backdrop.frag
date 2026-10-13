#version 440
// Window backdrop (Settings › Appearance › Background): the skin's colour plus
// up to three soft radial glows, or one diagonal three-stop wash. Computed in
// float and dithered before the 8-bit output, so the faint dark gradients do
// not band. Rebuild the .qsb with qml/shaders/build.sh after editing.

layout(location = 0) in vec2 qt_TexCoord0;
layout(location = 0) out vec4 fragColor;

layout(std140, binding = 0) uniform buf {
    mat4 qt_Matrix;
    float qt_Opacity;
    vec2 area;          // item size, px
    float radius;       // window corner radius, px
    float linearMode;   // 1 = diagonal wash (tone1..3 as stops), 0 = glows
    vec4 bg;            // skin background
    vec4 tone1;         // rgb + strength (alpha at the centre / at the stop)
    vec4 tone2;
    vec4 tone3;
    vec4 blob1;         // centre x, y and radius, px
    vec4 blob2;
    vec4 blob3;
};

float hash(vec2 p)
{
    vec3 p3 = fract(vec3(p.xyx) * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

vec3 glow(vec3 base, vec4 tone, vec4 blob, vec2 p)
{
    float t = clamp(length(p - blob.xy) / max(blob.z, 1.0), 0.0, 1.0);
    return mix(base, tone.rgb, tone.a * (1.0 - t));
}

void main()
{
    vec2 p = qt_TexCoord0 * area;
    vec3 c = bg.rgb;
    if (linearMode > 0.5) {
        float t = clamp(dot(p, area) / max(dot(area, area), 1.0), 0.0, 1.0);
        vec4 s = t < 0.55 ? mix(tone1, tone2, t / 0.55) : mix(tone2, tone3, (t - 0.55) / 0.45);
        c = mix(c, s.rgb, s.a);
    } else {
        c = glow(c, tone1, blob1, p);
        c = glow(c, tone2, blob2, p);
        c = glow(c, tone3, blob3, p);
    }

    // triangular dither of about one 8-bit step: hides the quantisation bands
    float n = hash(gl_FragCoord.xy) + hash(gl_FragCoord.xy + 17.17) - 1.0;
    c = clamp(c + n / 255.0, 0.0, 1.0);

    // rounded window corners, anti-aliased
    vec2 q = abs(p - 0.5 * area) - (0.5 * area - radius);
    float d = length(max(q, 0.0)) + min(max(q.x, q.y), 0.0) - radius;
    float mask = clamp(0.5 - d, 0.0, 1.0);

    fragColor = vec4(c, 1.0) * (mask * qt_Opacity);
}
