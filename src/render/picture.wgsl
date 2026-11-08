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

@fragment
fn fs_main(in: VertexOutput) -> @location(0) vec4<f32> {
    var texel = textureSample(picture, picture_sampler, texture_uv(in.uv, in.orientation));
    let decoded = in.tinted > 1.5;
    if decoded {
        texel = vec4<f32>(srgb_to_linear(texel.rgb), texel.a);
    }
    let tinted = vec4<f32>(in.tint.rgb, texel.a * in.tint.a);
    let color = mix(texel, tinted, select(in.tinted, 0.0, decoded));
    return vec4<f32>(color.rgb, color.a * in.opacity);
}
