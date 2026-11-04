//! Photoshop / Paint.NET style blend modes for compositing two layers (widget, mask).
//! Formulas follow Photocraft's `crates/color/src/blend.rs`. They run in a GSK GL shader,
//! so they need GTK's GL renderer (the default); without it the layers are just stacked.

use std::cell::OnceCell;

use gtk::prelude::*;
use gtk::{glib, graphene, gsk};
use serde::{Deserialize, Serialize};

/// The discriminant is the `u_mode` the shader switches on: keep both in sync.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "snake_case")]
pub enum BlendMode {
    #[default]
    Normal,
    Darken,
    Multiply,
    ColorBurn,
    LinearBurn,
    DarkerColor,
    Lighten,
    Screen,
    ColorDodge,
    LinearDodge,
    LighterColor,
    Overlay,
    SoftLight,
    HardLight,
    VividLight,
    LinearLight,
    PinLight,
    HardMix,
    Difference,
    Exclusion,
    Subtract,
    Divide,
    Hue,
    Saturation,
    Color,
    Luminosity,
    Reflect,
    Glow,
    Negation,
    Xor,
    PaintNetColorBurn,
    PaintNetColorDodge,
}

use BlendMode as B;

impl BlendMode {
    pub const ALL: [BlendMode; 32] = [
        B::Normal, B::Darken, B::Multiply, B::ColorBurn, B::LinearBurn, B::DarkerColor, B::Lighten, B::Screen,
        B::ColorDodge, B::LinearDodge, B::LighterColor, B::Overlay, B::SoftLight, B::HardLight, B::VividLight,
        B::LinearLight, B::PinLight, B::HardMix, B::Difference, B::Exclusion, B::Subtract, B::Divide, B::Hue,
        B::Saturation, B::Color, B::Luminosity, B::Reflect, B::Glow, B::Negation, B::Xor, B::PaintNetColorBurn,
        B::PaintNetColorDodge,
    ];

    pub fn label(self) -> &'static str {
        match self {
            B::Normal => "Normal",
            B::Darken => "Darken",
            B::Multiply => "Multiply",
            B::ColorBurn => "Color Burn",
            B::LinearBurn => "Linear Burn",
            B::DarkerColor => "Darker Color",
            B::Lighten => "Lighten",
            B::Screen => "Screen",
            B::ColorDodge => "Color Dodge",
            B::LinearDodge => "Linear Dodge (Add)",
            B::LighterColor => "Lighter Color",
            B::Overlay => "Overlay",
            B::SoftLight => "Soft Light",
            B::HardLight => "Hard Light",
            B::VividLight => "Vivid Light",
            B::LinearLight => "Linear Light",
            B::PinLight => "Pin Light",
            B::HardMix => "Hard Mix",
            B::Difference => "Difference",
            B::Exclusion => "Exclusion",
            B::Subtract => "Subtract",
            B::Divide => "Divide",
            B::Hue => "Hue",
            B::Saturation => "Saturation",
            B::Color => "Color",
            B::Luminosity => "Luminosity",
            B::Reflect => "Reflect",
            B::Glow => "Glow",
            B::Negation => "Negation",
            B::Xor => "XOR",
            B::PaintNetColorBurn => "Color Burn (Paint.NET)",
            B::PaintNetColorDodge => "Color Dodge (Paint.NET)",
        }
    }

    pub fn index(self) -> u32 {
        self as u32
    }

    pub fn from_index(i: u32) -> BlendMode {
        Self::ALL.get(i as usize).copied().unwrap_or_default()
    }
}

/// `u_texture1` = backdrop, `u_texture2` = source (both premultiplied); W3C general compositing formula.
const SHADER: &str = r#"
uniform int u_mode;
uniform sampler2D u_texture1;
uniform sampler2D u_texture2;

float cburn(float b, float s) { if (b >= 1.0) return 1.0; if (s <= 0.0) return 0.0; return 1.0 - min((1.0 - b) / s, 1.0); }
float cdodge(float b, float s) { if (b <= 0.0) return 0.0; if (s >= 1.0) return 1.0; return min(b / (1.0 - s), 1.0); }
float hlight(float b, float s) { if (s <= 0.5) return b * 2.0 * s; float t = 2.0 * s - 1.0; return b + t - b * t; }
float slight(float b, float s) {
  if (s <= 0.5) return 2.0 * b * s + b * b * (1.0 - 2.0 * s);
  float d = b <= 0.25 ? ((16.0 * b - 12.0) * b + 4.0) * b : sqrt(b);
  return 2.0 * b * (1.0 - s) + d * (2.0 * s - 1.0);
}
float vivid(float b, float s) {
  if (s <= 0.0) return 0.0; if (s >= 1.0) return 1.0;
  return s <= 0.5 ? cburn(b, 2.0 * s) : cdodge(b, 2.0 * (s - 0.5));
}
float hmix(float b, float s) {
  float v;
  if (s <= 0.5) { v = b >= 0.9999 ? 1.0 : (s <= 0.0 ? 0.0 : 1.0 - min((1.0 - b) / (2.0 * s), 1.0)); }
  else { v = b <= 0.0001 ? 0.0 : (s >= 1.0 ? 1.0 : min(b / (2.0 * (1.0 - s)), 1.0)); }
  return v >= 0.499999 ? 1.0 : 0.0;
}
float reflect_(float b, float s) { return s >= 1.0 ? 1.0 : min(b * b / (1.0 - s), 1.0); }

float ch(float b, float s) {
  int m = u_mode;
  if (m == 1) return min(b, s);
  if (m == 2) return b * s;
  if (m == 3) return cburn(b, s);
  if (m == 4) return max(b + s - 1.0, 0.0);
  if (m == 6) return max(b, s);
  if (m == 7) return b + s - b * s;
  if (m == 8) return cdodge(b, s);
  if (m == 9) return min(b + s, 1.0);
  if (m == 11) return hlight(s, b);
  if (m == 12) return slight(b, s);
  if (m == 13) return hlight(b, s);
  if (m == 14) return vivid(b, s);
  if (m == 15) return clamp(b + 2.0 * s - 1.0, 0.0, 1.0);
  if (m == 16) return s <= 0.5 ? min(b, 2.0 * s) : max(b, 2.0 * s - 1.0);
  if (m == 17) return hmix(b, s);
  if (m == 18) return abs(b - s);
  if (m == 19) return b + s - 2.0 * b * s;
  if (m == 20) return max(b - s, 0.0);
  if (m == 21) return s <= 0.0 ? (b <= 0.0 ? 0.0 : 1.0) : min(b / s, 1.0);
  if (m == 26) return reflect_(b, s);
  if (m == 27) return reflect_(s, b);
  if (m == 28) return 1.0 - abs(1.0 - b - s);
  if (m == 29) return float(int(floor(clamp(b, 0.0, 1.0) * 255.0 + 0.5)) ^ int(floor(clamp(s, 0.0, 1.0) * 255.0 + 0.5))) / 255.0;
  if (m == 30) return s <= 0.0 ? 0.0 : max(1.0 - (1.0 - b) / s, 0.0);
  if (m == 31) return s >= 1.0 ? 1.0 : min(b / (1.0 - s), 1.0);
  return s;
}

float lum(vec3 c) { return dot(c, vec3(0.3, 0.59, 0.11)); }
vec3 clipc(vec3 c) {
  float l = lum(c), n = min(min(c.r, c.g), c.b), x = max(max(c.r, c.g), c.b);
  if (n < 0.0) c = l > 0.0 ? l + (c - l) * l / (l - n) : vec3(l);
  if (x > 1.0) c = l < 1.0 ? l + (c - l) * (1.0 - l) / (x - l) : vec3(l);
  return c;
}
vec3 setlum(vec3 c, float l) { return clipc(c + (l - lum(c))); }
float sat(vec3 c) { return max(max(c.r, c.g), c.b) - min(min(c.r, c.g), c.b); }
vec3 setsat(vec3 c, float s) {
  float n = min(min(c.r, c.g), c.b), x = max(max(c.r, c.g), c.b);
  return x > n ? (c - n) * s / (x - n) : vec3(0.0);
}

vec3 mixrgb(vec3 b, vec3 s) {
  int m = u_mode;
  if (m == 0) return s;
  if (m == 5) return lum(s) < lum(b) ? s : b;
  if (m == 10) return lum(s) > lum(b) ? s : b;
  if (m == 22) return setlum(setsat(s, sat(b)), lum(b));
  if (m == 23) return setlum(setsat(b, sat(s)), lum(b));
  if (m == 24) return setlum(s, lum(b));
  if (m == 25) return setlum(b, lum(s));
  return vec3(ch(b.r, s.r), ch(b.g, s.g), ch(b.b, s.b));
}

void mainImage(out vec4 fragColor, in vec2 fragCoord, in vec2 resolution, in vec2 uv) {
  vec4 B = GskTexture(u_texture1, uv);
  vec4 S = GskTexture(u_texture2, uv);
  vec3 cb = B.a > 0.0 ? B.rgb / B.a : vec3(0.0);
  vec3 cs = S.a > 0.0 ? S.rgb / S.a : vec3(0.0);
  vec3 r = clamp(mixrgb(cb, cs), 0.0, 1.0);
  fragColor = vec4((1.0 - S.a) * B.rgb + (1.0 - B.a) * S.rgb + S.a * B.a * r, S.a + B.a * (1.0 - S.a));
}
"#;

thread_local! {
    /// Compiled once; `Some(None)` = the renderer cannot run GL shaders.
    static COMPILED: OnceCell<Option<gsk::GLShader>> = const { OnceCell::new() };
}

fn shader(renderer: Option<gsk::Renderer>) -> Option<gsk::GLShader> {
    COMPILED.with(|c| {
        if c.get().is_none() {
            let s = gsk::GLShader::from_bytes(&glib::Bytes::from_static(SHADER.as_bytes()));
            let ok = renderer.is_some_and(|r| s.compile(&r).map_err(|e| eprintln!("gifdeck: blend shader unavailable: {e}")).is_ok());
            let _ = c.set(ok.then_some(s));
        }
        c.get().cloned().flatten()
    })
}

/// Draw `top` over `bottom` inside `rect` with `mode`. Each layer is a closure that records its nodes.
pub fn draw(snapshot: &gtk::Snapshot, renderer: Option<gsk::Renderer>, mode: BlendMode, rect: &graphene::Rect, bottom: &dyn Fn(&gtk::Snapshot), top: &dyn Fn(&gtk::Snapshot)) {
    let shader = (mode != BlendMode::Normal).then(|| shader(renderer)).flatten();
    let Some(shader) = shader else {
        bottom(snapshot);
        top(snapshot);
        return;
    };
    let args = gsk::ShaderArgsBuilder::new(&shader, None);
    args.set_int(0, mode as i32);
    snapshot.push_gl_shader(&shader, rect, args.to_args());
    bottom(snapshot);
    snapshot.gl_shader_pop_texture();
    top(snapshot);
    snapshot.gl_shader_pop_texture();
    snapshot.pop();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn index_roundtrip_and_serde() {
        for (i, m) in BlendMode::ALL.iter().enumerate() {
            assert_eq!(m.index() as usize, i);
            assert_eq!(BlendMode::from_index(i as u32), *m);
            let j = serde_json::to_string(m).unwrap();
            assert_eq!(serde_json::from_str::<BlendMode>(&j).unwrap(), *m);
        }
    }
}
