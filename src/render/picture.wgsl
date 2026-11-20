// Pictures, one textured instance each, drawn as a four-vertex strip.

struct Viewport {
    // The target's size in physical pixels.
    size: vec2<f32>,
    // Unused: pads the uniform to 16 bytes.
    padding: vec2<f32>,
}

struct Instance {
    // Left, top, width, height, in physical pixels.
    @location(0) rect: vec4<f32>,
    // The color every texel takes, already in the target's color space.
    @location(1) tint: vec4<f32>,
    // x is 1 to draw in the tint, keeping the texel's alpha, 0 to draw the
    // texel's own color, and 2 to draw its own color decoded from sRGB to
    // linear (a texture the app rendered, sampled as stored); y multiplies the alpha (a fade); z is how many
    // quarter turns clockwise the picture is turned, and w is 1 to mirror
    // it left to right after.
    @location(2) mode: vec4<f32>,
    // The part of the texture shown in rect: left, top, width, height, as
    // fractions of the texture; all of it unless a clip cut the picture.
    @location(3) part: vec4<f32>,
}

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) tint: vec4<f32>,
    @location(2) tinted: f32,
    @location(3) opacity: f32,
    @location(4) orientation: vec2<f32>,
}

@group(0) @binding(0)
var<uniform> viewport: Viewport;

@group(1) @binding(0)
var picture: texture_2d<f32>;

@group(1) @binding(1)
var picture_sampler: sampler;

@vertex
fn vs_main(@builtin(vertex_index) vertex: u32, instance: Instance) -> VertexOutput {
    // Strip order: top left, top right, bottom left, bottom right.
    let corner = vec2<f32>(f32(vertex & 1u), f32(vertex >> 1u));
    let pixel = instance.rect.xy + corner * instance.rect.zw;
    let clip = vec2<f32>(
        pixel.x / viewport.size.x * 2.0 - 1.0,
        1.0 - pixel.y / viewport.size.y * 2.0,
    );
    var out: VertexOutput;
    out.position = vec4<f32>(clip, 0.0, 1.0);
    out.uv = instance.part.xy + corner * instance.part.zw;
    out.tint = instance.tint;
    out.tinted = instance.mode.x;
    out.opacity = instance.mode.y;
    out.orientation = instance.mode.zw;
    return out;
}

// Where in the texture the point `shown` of the drawn picture comes from,
// as fractions of each: unmirrored, then turned back.
fn texture_uv(shown: vec2<f32>, orientation: vec2<f32>) -> vec2<f32> {
    var f = shown;
    if orientation.y > 0.5 {
        f.x = 1.0 - f.x;
    }
    let turns = u32(orientation.x + 0.5) % 4u;
    if turns == 1u {
        return vec2<f32>(f.y, 1.0 - f.x);
    }
    if turns == 2u {
        return vec2<f32>(1.0 - f.x, 1.0 - f.y);
    }
    if turns == 3u {
        return vec2<f32>(1.0 - f.y, f.x);
    }
    return f;
}

// The linear value of the sRGB-encoded channels `encoded`.
fn srgb_to_linear(encoded: vec3<f32>) -> vec3<f32> {
    let low = encoded / 12.92;
    let high = pow((encoded + 0.055) / 1.055, vec3<f32>(2.4));
    return select(high, low, encoded <= vec3<f32>(0.04045));
}

// The picture's color at `uv`, turned and mirrored by `orientation`,
// drawn in its own colors or `tint` as `tinted` says, and faded by
// `opacity`.
fn picture_color(
    uv: vec2<f32>,
    orientation: vec2<f32>,
    tint: vec4<f32>,
    tinted: f32,
    opacity: f32,
) -> vec4<f32> {
    var texel = textureSample(picture, picture_sampler, texture_uv(uv, orientation));
    let decoded = tinted > 1.5;
    if decoded {
        texel = vec4<f32>(srgb_to_linear(texel.rgb), texel.a);
    }
    let tinted_texel = vec4<f32>(tint.rgb, texel.a * tint.a);
    let color = mix(texel, tinted_texel, select(tinted, 0.0, decoded));
    return vec4<f32>(color.rgb, color.a * opacity);
}

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    return picture_color(in.uv, in.orientation, in.tint, in.tinted, in.opacity);
}

// A picture drawn inside a transform: its rect and part are in coordinates
// of its own, which the placement maps to window pixels, and the window
// clip is in window pixels.
struct Placement {
    // The matrix's a, b, c, d: how the picture's x and y add to the
    // window's.
    @location(4) matrix: vec4<f32>,
    // The matrix's e and f, what is added; window pixels per unit of the
    // picture's own; and 1 when the matrix turns rectangles off the axes.
    @location(5) shift: vec4<f32>,
    // Left, top, width, height of the window area outside which nothing is
    // drawn.
    @location(6) window_clip: vec4<f32>,
}

struct PlacedOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
    @location(1) @interpolate(flat) tint: vec4<f32>,
    @location(2) @interpolate(flat) tinted: f32,
    @location(3) @interpolate(flat) opacity: f32,
    @location(4) @interpolate(flat) orientation: vec2<f32>,
    // Where the pixel is in the picture's own coordinates.
    @location(5) local: vec2<f32>,
    @location(6) @interpolate(flat) rect: vec4<f32>,
    @location(7) @interpolate(flat) window_clip: vec4<f32>,
    // Window pixels per unit, and 1 when turned.
    @location(8) @interpolate(flat) scale_turned: vec2<f32>,
}

@vertex
fn vs_placed(
    @builtin(vertex_index) vertex: u32,
    instance: Instance,
    placement: Placement,
) -> PlacedOutput {
    // A turned picture is grown by a pixel, in its own units, for its
    // antialiased edge; the texture's edge texels stretch over it.
    let scale = max(placement.shift.z, 1e-6);
    let turned = placement.shift.w > 0.5;
    let grow = select(0.0, 1.0 / scale, turned);
    let corner = vec2<f32>(f32(vertex & 1u), f32(vertex >> 1u));
    let size = max(instance.rect.zw, vec2<f32>(1e-6));
    let local = instance.rect.xy - vec2<f32>(grow) + corner * (size + vec2<f32>(2.0 * grow));
    let pixel = vec2<f32>(
        placement.matrix.x * local.x + placement.matrix.z * local.y + placement.shift.x,
        placement.matrix.y * local.x + placement.matrix.w * local.y + placement.shift.y,
    );
    let clip = vec2<f32>(
        pixel.x / viewport.size.x * 2.0 - 1.0,
        1.0 - pixel.y / viewport.size.y * 2.0,
    );
    var out: PlacedOutput;
    out.position = vec4<f32>(clip, 0.0, 1.0);
    out.uv = instance.part.xy + (local - instance.rect.xy) / size * instance.part.zw;
    out.tint = instance.tint;
    out.tinted = instance.mode.x;
    out.opacity = instance.mode.y;
    out.orientation = instance.mode.zw;
    out.local = local;
    out.rect = instance.rect;
    out.window_clip = placement.window_clip;
    out.scale_turned = vec2<f32>(scale, placement.shift.w);
    return out;
}

@fragment
fn fs_placed(in: PlacedOutput) -> @location(0) vec4<f32> {
    // Sampled first, while every pixel of the strip still runs.
    let uv = clamp(in.uv, vec2<f32>(0.0), vec2<f32>(1.0));
    let color = picture_color(uv, in.orientation, in.tint, in.tinted, in.opacity);
    let window = in.position.xy;
    if any(window < in.window_clip.xy) || any(window >= in.window_clip.xy + in.window_clip.zw) {
        discard;
    }
    if in.scale_turned.y < 0.5 {
        return color;
    }
    // The distance outside the rect, in window pixels, fades a pixel
    // wide ramp across its edge.
    let half = in.rect.zw * 0.5;
    let q = abs(in.local - (in.rect.xy + half)) - half;
    let distance = length(max(q, vec2<f32>(0.0))) + min(max(q.x, q.y), 0.0);
    let cover = clamp(0.5 - distance * in.scale_turned.x, 0.0, 1.0);
    return vec4<f32>(color.rgb, color.a * cover);
}
