//! GLSL sources (GLSL 1.20 for broad driver support).

/// Full-screen triangle-free quad: vertices are given in clip space.
pub const VERTEX: &str = r#"#version 120
varying vec2 uv;
void main() {
    gl_Position = vec4(gl_Vertex.xy, 0.0, 1.0);
    uv = gl_Vertex.xy * 0.5 + 0.5;
}
"#;

/// Final pass: contrast-adaptive sharpening (after AMD FidelityFX CAS),
/// then colour grading (contrast, saturation, vibrance, warmth) with a
/// gentle filmic S-curve.
pub const FINAL: &str = r#"#version 120
uniform sampler2D colorTex;
uniform vec2 texel;
uniform float sharpen;     // 0..1
uniform float contrast;    // 1 = neutral
uniform float saturation;  // 1 = neutral
uniform float vibrance;    // 0 = neutral
uniform float warmth;      // 0 = neutral, + warmer, - cooler
uniform float filmic;      // 0..1 strength of the S-curve
varying vec2 uv;

vec3 tap(float x, float y) { return texture2D(colorTex, uv + vec2(x, y) * texel).rgb; }

vec3 cas(vec3 e) {
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
}

void main() {
    vec3 col = texture2D(colorTex, uv).rgb;
    if (sharpen > 0.0) col = cas(col);

    // Contrast around mid grey.
    col = (col - 0.5) * contrast + 0.5;
    // Saturation and vibrance (boosts muted colours more than vivid ones).
    float luma = dot(col, vec3(0.2126, 0.7152, 0.0722));
    col = mix(vec3(luma), col, saturation);
    float sat = max(col.r, max(col.g, col.b)) - min(col.r, min(col.g, col.b));
    col = mix(vec3(luma), col, 1.0 + vibrance * (1.0 - sat));
    // Colour temperature.
    col *= vec3(1.0 + 0.06 * warmth, 1.0 + 0.01 * warmth, 1.0 - 0.06 * warmth);
    // Gentle filmic S-curve (soft toe and shoulder).
    col = clamp(col, 0.0, 1.0);
    vec3 s = col * col * (3.0 - 2.0 * col);
    col = mix(col, s, filmic);

    gl_FragColor = vec4(col, 1.0);
}
"#;
