// Vector paths, tessellated on the CPU into triangles whose corners carry
// how much of a pixel the path covers there (1 inside, fading to 0 across
// a pixel wide fringe at the edge), drawn in one color or a gradient read
// from a ramp texture, one row per gradient.

struct Viewport {
    // The target's size in physical pixels.
    size: vec2<f32>,
    // Unused: pads the uniform to 16 bytes.
    padding: vec2<f32>,
}

struct Vertex {
    // Position in physical pixels, coverage, and where along the gradient.
    @location(0) position_coverage_gradient: vec4<f32>,
    // Already in the target's color space, straight alpha; white with the
    // path's opacity for a gradient, which the ramp colors.
    @location(1) color: vec4<f32>,
    // Left, top, width, height of the area outside which nothing is drawn.
    @location(2) clip: vec4<f32>,
    // The gradient's row in the ramp texture, or below 0 for one color.
    @location(3) ramp: f32,
}

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) coverage: f32,
    @location(1) gradient: f32,
    @location(2) @interpolate(flat) color: vec4<f32>,
    @location(3) @interpolate(flat) clip: vec4<f32>,
    @location(4) @interpolate(flat) ramp: f32,
}

@group(0) @binding(0)
var<uniform> viewport: Viewport;

// One row of texels per gradient, already in the target's color space.
@group(0) @binding(1)
var ramps: texture_2d<f32>;

@vertex
fn vs_main(vertex: Vertex) -> VertexOutput {
    let pixel = vertex.position_coverage_gradient.xy;
    let clip = vec2<f32>(
        pixel.x / viewport.size.x * 2.0 - 1.0,
        1.0 - pixel.y / viewport.size.y * 2.0,
    );
    var out: VertexOutput;
    out.position = vec4<f32>(clip, 0.0, 1.0);
    out.coverage = vertex.position_coverage_gradient.z;
    out.gradient = vertex.position_coverage_gradient.w;
    out.color = vertex.color;
    out.clip = vertex.clip;
    out.ramp = vertex.ramp;
    return out;
}

// The gradient's color at `offset` along row `row`, blending the two
// nearest texels (the texture is not filterable).
fn ramp_color(row: i32, offset: f32) -> vec4<f32> {
    let last = f32(textureDimensions(ramps).x - 1u);
    let at = clamp(offset, 0.0, 1.0) * last;
    let low = floor(at);
    let high = min(low + 1.0, last);
    let a = textureLoad(ramps, vec2<i32>(i32(low), row), 0);
    let b = textureLoad(ramps, vec2<i32>(i32(high), row), 0);
    return mix(a, b, at - low);
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    let p = in.position.xy;
    if any(p < in.clip.xy) || any(p >= in.clip.xy + in.clip.zw) {
        discard;
    }
    var color = in.color;
    if in.ramp >= 0.0 {
        color = color * ramp_color(i32(in.ramp + 0.5), in.gradient);
    }
    let alpha = color.a * clamp(in.coverage, 0.0, 1.0);
    if alpha <= 0.0 {
        discard;
    }
    return vec4<f32>(color.rgb, alpha);
}
