// The window's rounded corners, cut out of what a frame drew: one triangle
// over the whole target whose pixels scale what is under them by how much
// of each lies inside the window's rounded rectangle, so the corners turn
// see-through and their curve is antialiased.

struct Mask {
    // The target's size in physical pixels.
    size: vec2<f32>,
    // The corners' radius in physical pixels.
    radius: f32,
    // Unused: pads the uniform to 16 bytes.
    padding: f32,
}

@group(0) @binding(0)
var<uniform> mask: Mask;

@vertex
fn vs_main(@builtin(vertex_index) vertex: u32) -> @builtin(position) vec4<f32> {
    // A triangle reaching past the target's corners covers all of it.
    let corner = vec2<f32>(f32((vertex << 1u) & 2u), f32(vertex & 2u));
    return vec4<f32>(corner * 2.0 - 1.0, 0.0, 1.0);
}

// The signed distance from `p` to the edge of a box centered at the origin
// with half size `half` and corners rounded by `radius`: negative inside.
fn rounded_box_distance(p: vec2<f32>, half: vec2<f32>, radius: f32) -> f32 {
    let q = abs(p) - (half - vec2<f32>(radius));
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - radius;
}

@fragment
fn fs_main(@builtin(position) position: vec4<f32>) -> @location(0) vec4<f32> {
    let half = mask.size * 0.5;
    let distance = rounded_box_distance(position.xy - half, half, mask.radius);
    // The blend multiplies what is under the pixel by this alpha.
    return vec4<f32>(0.0, 0.0, 0.0, clamp(0.5 - distance, 0.0, 1.0));
}
