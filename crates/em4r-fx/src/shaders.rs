//! GLSL sources (GLSL 1.20 for broad driver support).

/// Full-screen quad: vertices are given in clip space.
pub const VERTEX: &str = r#"#version 120
varying vec2 uv;
void main() {
    gl_Position = vec4(gl_Vertex.xy, 0.0, 1.0);
    uv = gl_Vertex.xy * 0.5 + 0.5;
}
"#;

/// Shared helpers: linear depth and view-space position from the depth
/// buffer (camera looks along +z here, z = distance in world units).
const DEPTH_HELPERS: &str = r#"
uniform sampler2D depthTex;
uniform vec2 nearFar;   // world camera near / far
uniform vec2 tanHalf;   // tan(half fov) x / y
float rawDepth(vec2 c) { return texture2D(depthTex, c).r; }
float linZ(vec2 c) {
    float n = nearFar.x, f = nearFar.y;
    float z = rawDepth(c) * 2.0 - 1.0;
    return 2.0 * n * f / (f + n - z * (f - n));
}
vec3 viewPos(vec2 c) {
    float z = linZ(c);
    return vec3((c * 2.0 - 1.0) * tanHalf * z, z);
}
"#;

/// Screen-space ambient occlusion (normal-oriented hemisphere test on a
/// per-pixel rotated spiral). Output: occlusion factor in .r (1 = open).
pub fn ssao() -> String {
    format!(
        r#"#version 120
{DEPTH_HELPERS}
uniform vec2 texel;
uniform float radius;    // world units
uniform float focal;     // pixels per world unit at distance 1
varying vec2 uv;
void main() {{
    if (rawDepth(uv) >= 0.99999) {{ gl_FragColor = vec4(1.0); return; }}
    vec3 P = viewPos(uv);
    vec3 N = normalize(cross(dFdx(P), dFdy(P)));
    if (N.z > 0.0) N = -N;                       // face the camera
    float rPix = clamp(radius * focal / P.z, 2.0, 90.0);
    float rot = fract(sin(dot(gl_FragCoord.xy, vec2(12.9898, 78.233))) * 43758.5453) * 6.2831853;
    float occ = 0.0;
    const int S = 12;
    for (int i = 0; i < S; i++) {{
        float t = (float(i) + 0.5) / float(S);
        float a = rot + t * 6.2831853 * 3.0;    // three turns
        vec2 c = uv + vec2(cos(a), sin(a)) * (t * rPix) * texel;
        vec3 v = viewPos(c) - P;
        float len = length(v);
        float range = smoothstep(0.0, 1.0, radius * 2.0 / max(len, 1e-3) - 1.0); // fade far samples
        occ += max(0.0, dot(v / max(len, 1e-3), N) - 0.15) * range;
    }}
    float ao = clamp(1.0 - occ / float(S) * 1.6, 0.0, 1.0);
    gl_FragColor = vec4(ao, ao, ao, 1.0);
}}
"#
    )
}

/// Final pass: depth-aware AO blur, distance depth of field, contrast-
/// adaptive sharpening (after AMD FidelityFX CAS) and colour grading.
pub fn final_pass() -> String {
    format!(
        r#"#version 120
{DEPTH_HELPERS}
uniform sampler2D colorTex;
uniform sampler2D aoTex;
uniform vec2 texel;
uniform float useAO;       // 0 / 1
uniform float aoStrength;  // 0..2
uniform float useDOF;      // 0 / 1
uniform float dofStrength; // 0..1
uniform vec2 dofRange;     // start, end distance
uniform float sharpen;     // 0..1
uniform float contrast;    // 1 = neutral
uniform float saturation;  // 1 = neutral
uniform float vibrance;    // 0 = neutral
uniform float warmth;      // 0 = neutral, + warmer, - cooler
uniform float filmic;      // 0..1 strength of the S-curve
varying vec2 uv;

vec3 tap(float x, float y) {{ return texture2D(colorTex, uv + vec2(x, y) * texel).rgb; }}

vec3 cas(vec3 e) {{
    vec3 a = tap(-1.0,-1.0), b = tap(0.0,-1.0), c = tap(1.0,-1.0);
    vec3 d = tap(-1.0, 0.0),                    f = tap(1.0, 0.0);
    vec3 g = tap(-1.0, 1.0), h = tap(0.0, 1.0), i = tap(1.0, 1.0);
    vec3 mn = min(min(min(d, e), min(f, b)), h);
    vec3 mn2 = min(mn, min(min(a, c), min(g, i)));
    mn += mn2;
    vec3 mx = max(max(max(d, e), max(f, b)), h);
    vec3 mx2 = max(mx, max(max(a, c), max(g, i)));
    mx += mx2;
    vec3 amp = clamp(min(mn, 2.0 - mx) / max(mx, vec3(1e-4)), 0.0, 1.0);
    amp = sqrt(amp);
    float peak = -1.0 / mix(8.0, 5.0, sharpen);
    vec3 w = amp * peak;
    vec3 rcpW = 1.0 / (1.0 + 4.0 * w);
    return clamp(((b + d + f + h) * w + e) * rcpW, 0.0, 1.0);
}}

float blurredAO(float z0) {{
    float sum = 0.0, wsum = 0.0;
    for (int x = -2; x <= 2; x++) {{
        for (int y = -2; y <= 2; y++) {{
            vec2 c = uv + vec2(float(x), float(y)) * texel * 1.5;
            float w = 1.0 / (1.0 + abs(linZ(c) - z0) / (0.03 * z0));
            sum += texture2D(aoTex, c).r * w;
            wsum += w;
        }}
    }}
    return sum / wsum;
}}

void main() {{
    vec3 base = texture2D(colorTex, uv).rgb;
    vec3 col = sharpen > 0.0 ? cas(base) : base;
    bool sky = rawDepth(uv) >= 0.99999;
    float z = linZ(uv);

    if (useDOF > 0.5 && !sky) {{
        float amt = dofStrength * smoothstep(dofRange.x, dofRange.y, z);
        if (amt > 0.01) {{
            vec3 acc = vec3(0.0);
            const int K = 12;
            for (int i = 0; i < K; i++) {{
                float a = float(i) * 2.3999632;           // golden angle
                float r = sqrt((float(i) + 0.5) / float(K)) * amt * 6.0;
                acc += texture2D(colorTex, uv + vec2(cos(a), sin(a)) * r * texel).rgb;
            }}
            col = mix(col, acc / float(K), clamp(amt * 1.5, 0.0, 1.0));
        }}
    }}
    if (useAO > 0.5 && !sky) {{
        float ao = blurredAO(z);
        col *= pow(ao, clamp(aoStrength, 0.0, 2.0));   // 0 = off, 1 = normal, 2 = strong
    }}

    // Colour grading.
    col = (col - 0.5) * contrast + 0.5;
    float luma = dot(col, vec3(0.2126, 0.7152, 0.0722));
    col = mix(vec3(luma), col, saturation);
    float sat = max(col.r, max(col.g, col.b)) - min(col.r, min(col.g, col.b));
    col = mix(vec3(luma), col, 1.0 + vibrance * (1.0 - sat));
    col *= vec3(1.0 + 0.06 * warmth, 1.0 + 0.01 * warmth, 1.0 - 0.06 * warmth);
    col = clamp(col, 0.0, 1.0);
    vec3 s = col * col * (3.0 - 2.0 * col);
    col = mix(col, s, filmic);

    gl_FragColor = vec4(col, 1.0);
}}
"#
    )
}
