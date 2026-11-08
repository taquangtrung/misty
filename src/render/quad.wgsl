// Rectangles, one instance each, drawn as a four-vertex strip and shaded by
// each pixel's distance to the edge: plain, or with rounded corners, a
// border along the inside of the edge, or the edge faded over a blur.

struct Viewport {
    // The target's size in physical pixels.
    size: vec2<f32>,
    // Unused: pads the uniform to 16 bytes.
    padding: vec2<f32>,
}

struct Instance {
    // Left, top, width, height, in physical pixels.
    @location(0) rect: vec4<f32>,
    // Already in the target's color space, straight alpha.
    @location(1) color: vec4<f32>,
    // The border's color, in the same space; unseen when its width is 0.
    @location(2) border_color: vec4<f32>,
    // Corner radius, border width, blur, and padding, in physical pixels.
    @location(3) shape: vec4<f32>,
    // Left, top, width, height of the area outside which nothing is drawn.
    @location(4) clip: vec4<f32>,
}

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) @interpolate(flat) rect: vec4<f32>,
    @location(1) @interpolate(flat) color: vec4<f32>,
    @location(2) @interpolate(flat) border_color: vec4<f32>,
    @location(3) @interpolate(flat) shape: vec4<f32>,
    @location(4) @interpolate(flat) clip: vec4<f32>,
}

@group(0) @binding(0)
var<uniform> viewport: Viewport;

@vertex
fn vs_main(@builtin(vertex_index) vertex: u32, instance: Instance) -> VertexOutput {
    // A rounded or blurred quad is grown to cover its softened edge: the
    // blur's reach and a pixel of antialiasing. A plain one covers just the
    // pixels whose centers it holds, as a solid fill always has.
    let radius = instance.shape.x;
    let blur = max(instance.shape.z, 0.0);
    let grow = select(0.0, blur + 1.0, radius > 0.0 || blur > 0.0);
    // Strip order: top left, top right, bottom left, bottom right.
    let corner = vec2<f32>(f32(vertex & 1u), f32(vertex >> 1u));
    let origin = instance.rect.xy - vec2<f32>(grow);
    let size = instance.rect.zw + vec2<f32>(2.0 * grow);
    let pixel = origin + corner * size;
    let clip = vec2<f32>(
        pixel.x / viewport.size.x * 2.0 - 1.0,
        1.0 - pixel.y / viewport.size.y * 2.0,
    );
    var out: VertexOutput;
    out.position = vec4<f32>(clip, 0.0, 1.0);
    out.rect = instance.rect;
    out.color = instance.color;
    out.border_color = instance.border_color;
    out.shape = instance.shape;
    out.clip = instance.clip;
    return out;
}

// The signed distance from `p` to the edge of a box centered at the origin
// with half size `half` and corners rounded by `radius`: negative inside.
fn rounded_box_distance(p: vec2<f32>, half: vec2<f32>, radius: f32) -> f32 {
    let q = abs(p) - (half - vec2<f32>(radius));
    return length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0) - radius;
}

// How much of the pixel at `distance` from an edge is inside it: a pixel
// wide ramp when `soft`, all or nothing at the pixel's center otherwise.
fn coverage(distance: f32, soft: bool) -> f32 {
    if soft {
        return clamp(0.5 - distance, 0.0, 1.0);
    }
    return select(0.0, 1.0, distance <= 0.0);
}

fn premultiply(color: vec4<f32>) -> vec4<f32> {
    return vec4<f32>(color.rgb * color.a, color.a);
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    // The center of the pixel being shaded.
    let p = in.position.xy;
    if any(p < in.clip.xy) || any(p >= in.clip.xy + in.clip.zw) {
        discard;
    }
    let half = in.rect.zw * 0.5;
    let radius = clamp(in.shape.x, 0.0, min(half.x, half.y));
    let border = max(in.shape.y, 0.0);
    let blur = max(in.shape.z, 0.0);
    let distance = rounded_box_distance(p - (in.rect.xy + half), half, radius);
    if blur > 0.0 {
        let fade = 1.0 - smoothstep(-blur, blur, distance);
        return vec4<f32>(in.color.rgb, in.color.a * fade);
    }
    let soft = radius > 0.0;
    let outer = coverage(distance, soft);
    // Inside the border's inner edge the fill shows; between it and the
    // outer edge, the border. Mixed premultiplied, so a transparent fill
    // leaves the border alone rather than darkening it.
    let inner = select(1.0, coverage(distance + border, soft), border > 0.0);
    let mixed = mix(premultiply(in.border_color), premultiply(in.color), inner) * outer;
    if mixed.a <= 0.0 {
        discard;
    }
    return vec4<f32>(mixed.rgb / mixed.a, mixed.a);
}
