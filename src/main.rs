#![allow(dead_code)] // Keep the old CPU path around as a reference/fallback while the GPU path settles.

use glow::HasContext;
use sdl2::event::{Event, WindowEvent};
use sdl2::keyboard::{Keycode, Mod};
use sdl2::video::{FullscreenType, GLProfile};
use serde::Deserialize;
use std::f32::consts::PI;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

const DEFAULT_W: usize = 960;
const DEFAULT_H: usize = 640;
const LUT_N: usize = 65_536;
const PAL_N: usize = 1024;
const PROFILE_N: usize = 4000;
const PROFILE_SCALE: f32 = 3998.0;
const INV_SQRT2: f32 = 0.70710678118;
const POST_AA_OFFSETS: &[(f32, f32)] = &[(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)];

fn main() {
    let sdl = sdl2::init().expect("sdl init");
    let video = sdl.video().expect("sdl video");
    let gl_attr = video.gl_attr();
    gl_attr.set_context_profile(GLProfile::Core);
    gl_attr.set_context_version(3, 3);
    gl_attr.set_double_buffer(true);

    let mut window = video
        .window(
            "Gold Lace clean-room Rust",
            DEFAULT_W as u32,
            DEFAULT_H as u32,
        )
        .position_centered()
        .resizable()
        .allow_highdpi()
        .opengl()
        .build()
        .expect("window");
    let _gl_context = window.gl_create_context().expect("opengl context");
    window
        .gl_make_current(&_gl_context)
        .expect("make gl current");
    video.gl_set_swap_interval(1).ok();
    let gl = unsafe {
        glow::Context::from_loader_function(|name| video.gl_get_proc_address(name) as *const _)
    };
    let mut event_pump = sdl.event_pump().expect("events");
    let mut fullscreen = true;
    if let Err(e) = window.set_fullscreen(FullscreenType::Desktop) {
        fullscreen = false;
        eprintln!("initial fullscreen failed: {e}");
    } else {
        eprintln!("fullscreen: on");
    }

    let (out_w, out_h) = drawable_size(&window);
    let mut app = App::new(out_w, out_h);
    let mut renderer = unsafe { GpuRenderer::new(gl, out_w, out_h).expect("gpu renderer") };
    unsafe {
        renderer.upload_profile(&app.state);
        renderer.upload_palette(app.current_palette());
    }
    eprintln!("render size: {}x{}", out_w, out_h);

    'running: loop {
        for event in event_pump.poll_iter() {
            match event {
                Event::Quit { .. } => break 'running,
                Event::KeyDown {
                    keycode: Some(Keycode::Escape),
                    ..
                }
                | Event::KeyDown {
                    keycode: Some(Keycode::Q),
                    ..
                } => break 'running,
                Event::KeyDown {
                    keycode: Some(Keycode::F),
                    repeat: false,
                    ..
                } => {
                    fullscreen = !fullscreen;
                    let mode = if fullscreen {
                        FullscreenType::Desktop
                    } else {
                        FullscreenType::Off
                    };
                    if let Err(e) = window.set_fullscreen(mode) {
                        eprintln!("fullscreen toggle failed: {e}");
                    } else {
                        eprintln!("fullscreen: {}", if fullscreen { "on" } else { "off" });
                    }
                }
                Event::KeyDown {
                    keycode: Some(Keycode::Space),
                    keymod,
                    repeat: false,
                    ..
                } => {
                    if is_shift_down(keymod) {
                        app.prev_pattern();
                    } else {
                        app.next_pattern();
                    }
                    unsafe { renderer.upload_profile(&app.state) };
                }
                Event::KeyDown {
                    keycode: Some(Keycode::N),
                    repeat: false,
                    ..
                } => {
                    app.new_pattern();
                    unsafe { renderer.upload_profile(&app.state) };
                }
                Event::KeyDown {
                    keycode: Some(Keycode::P),
                    repeat: false,
                    ..
                } => app.paused = !app.paused,
                Event::KeyDown {
                    keycode: Some(Keycode::R),
                    repeat: false,
                    ..
                } => {
                    app.random_palette();
                    unsafe { renderer.upload_palette(app.current_palette()) };
                }
                Event::KeyDown {
                    keycode: Some(Keycode::LeftBracket),
                    repeat: false,
                    ..
                } => {
                    app.prev_palette();
                    unsafe { renderer.upload_palette(app.current_palette()) };
                }
                Event::KeyDown {
                    keycode: Some(Keycode::RightBracket),
                    repeat: false,
                    ..
                } => {
                    app.next_palette();
                    unsafe { renderer.upload_palette(app.current_palette()) };
                }
                Event::Window {
                    win_event: WindowEvent::Resized(_, _),
                    ..
                }
                | Event::Window {
                    win_event: WindowEvent::SizeChanged(_, _),
                    ..
                } => {
                    // SDL reports logical window size here; below we query the real
                    // renderer drawable size so Hi-DPI and fullscreen render natively.
                }
                _ => {}
            }
        }

        let (dw, dh) = drawable_size(&window);
        if app.resize(dw, dh) {
            unsafe { renderer.resize(dw, dh) };
            eprintln!("render size: {}x{}", dw, dh);
        }

        if !app.paused {
            app.palette_scroll += 0.45;
        }
        unsafe { renderer.render(&app) };
        window.gl_swap_window();
    }
}

struct GpuRenderer {
    gl: glow::Context,
    vao: glow::NativeVertexArray,
    scalar_program: glow::NativeProgram,
    color_program: glow::NativeProgram,
    framebuffer: glow::NativeFramebuffer,
    scalar_tex: glow::NativeTexture,
    profile_tex: glow::NativeTexture,
    palette_tex: glow::NativeTexture,
    w: usize,
    h: usize,
}

impl GpuRenderer {
    unsafe fn new(gl: glow::Context, w: usize, h: usize) -> Result<Self, String> {
        let vao = gl.create_vertex_array()?;
        let scalar_program = compile_program(&gl, FULLSCREEN_VS, SCALAR_FS)?;
        let color_program = compile_program(&gl, FULLSCREEN_VS, COLOR_FS)?;
        let framebuffer = gl.create_framebuffer()?;
        let scalar_tex = gl.create_texture()?;
        let profile_tex = gl.create_texture()?;
        let palette_tex = gl.create_texture()?;

        let mut renderer = Self {
            gl,
            vao,
            scalar_program,
            color_program,
            framebuffer,
            scalar_tex,
            profile_tex,
            palette_tex,
            w: 1,
            h: 1,
        };
        renderer.init_static_textures();
        renderer.resize(w, h);
        Ok(renderer)
    }

    unsafe fn init_static_textures(&self) {
        self.gl
            .bind_texture(glow::TEXTURE_2D, Some(self.profile_tex));
        self.gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_MIN_FILTER,
            glow::LINEAR as i32,
        );
        self.gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_MAG_FILTER,
            glow::LINEAR as i32,
        );
        self.gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_WRAP_S,
            glow::CLAMP_TO_EDGE as i32,
        );
        self.gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_WRAP_T,
            glow::CLAMP_TO_EDGE as i32,
        );

        self.gl
            .bind_texture(glow::TEXTURE_2D, Some(self.palette_tex));
        self.gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_MIN_FILTER,
            glow::LINEAR as i32,
        );
        self.gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_MAG_FILTER,
            glow::LINEAR as i32,
        );
        self.gl
            .tex_parameter_i32(glow::TEXTURE_2D, glow::TEXTURE_WRAP_S, glow::REPEAT as i32);
        self.gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_WRAP_T,
            glow::CLAMP_TO_EDGE as i32,
        );
    }

    unsafe fn resize(&mut self, w: usize, h: usize) {
        self.w = w.max(1);
        self.h = h.max(1);
        self.gl
            .bind_texture(glow::TEXTURE_2D, Some(self.scalar_tex));
        self.gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_MIN_FILTER,
            glow::LINEAR as i32,
        );
        self.gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_MAG_FILTER,
            glow::LINEAR as i32,
        );
        self.gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_WRAP_S,
            glow::CLAMP_TO_EDGE as i32,
        );
        self.gl.tex_parameter_i32(
            glow::TEXTURE_2D,
            glow::TEXTURE_WRAP_T,
            glow::CLAMP_TO_EDGE as i32,
        );
        self.gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::R32F as i32,
            self.w as i32,
            self.h as i32,
            0,
            glow::RED,
            glow::FLOAT,
            glow::PixelUnpackData::Slice(None),
        );

        self.gl
            .bind_framebuffer(glow::FRAMEBUFFER, Some(self.framebuffer));
        self.gl.framebuffer_texture_2d(
            glow::FRAMEBUFFER,
            glow::COLOR_ATTACHMENT0,
            glow::TEXTURE_2D,
            Some(self.scalar_tex),
            0,
        );
        if self.gl.check_framebuffer_status(glow::FRAMEBUFFER) != glow::FRAMEBUFFER_COMPLETE {
            eprintln!("warning: scalar framebuffer is incomplete");
        }
        self.gl.bind_framebuffer(glow::FRAMEBUFFER, None);
    }

    unsafe fn upload_profile(&self, state: &State) {
        self.gl
            .bind_texture(glow::TEXTURE_2D, Some(self.profile_tex));
        self.gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::R32F as i32,
            PROFILE_N as i32,
            1,
            0,
            glow::RED,
            glow::FLOAT,
            glow::PixelUnpackData::Slice(Some(bytemuck::cast_slice(&state.profile))),
        );
    }

    unsafe fn upload_palette(&self, palette: &Palette) {
        let mut rgba = Vec::with_capacity(PAL_N * 4);
        for &px in &palette.dense {
            rgba.push(((px >> 16) & 255) as u8);
            rgba.push(((px >> 8) & 255) as u8);
            rgba.push((px & 255) as u8);
            rgba.push(255);
        }
        self.gl
            .bind_texture(glow::TEXTURE_2D, Some(self.palette_tex));
        self.gl.tex_image_2d(
            glow::TEXTURE_2D,
            0,
            glow::RGBA8 as i32,
            PAL_N as i32,
            1,
            0,
            glow::RGBA,
            glow::UNSIGNED_BYTE,
            glow::PixelUnpackData::Slice(Some(&rgba)),
        );
    }

    unsafe fn render(&self, app: &App) {
        self.gl.disable(glow::DEPTH_TEST);
        self.gl.disable(glow::BLEND);
        self.gl.bind_vertex_array(Some(self.vao));

        self.gl
            .bind_framebuffer(glow::FRAMEBUFFER, Some(self.framebuffer));
        self.gl.viewport(0, 0, self.w as i32, self.h as i32);
        self.gl.use_program(Some(self.scalar_program));
        self.gl.active_texture(glow::TEXTURE0);
        self.gl
            .bind_texture(glow::TEXTURE_2D, Some(self.profile_tex));
        set_i32(&self.gl, self.scalar_program, "uProfile", 0);
        set_vec2(
            &self.gl,
            self.scalar_program,
            "uResolution",
            self.w as f32,
            self.h as f32,
        );
        self.set_state_uniforms(app, self.scalar_program);
        self.gl.draw_arrays(glow::TRIANGLES, 0, 3);

        self.gl.bind_framebuffer(glow::FRAMEBUFFER, None);
        self.gl.viewport(0, 0, self.w as i32, self.h as i32);
        self.gl.use_program(Some(self.color_program));
        self.gl.active_texture(glow::TEXTURE0);
        self.gl
            .bind_texture(glow::TEXTURE_2D, Some(self.scalar_tex));
        self.gl.active_texture(glow::TEXTURE1);
        self.gl
            .bind_texture(glow::TEXTURE_2D, Some(self.palette_tex));
        set_i32(&self.gl, self.color_program, "uScalar", 0);
        set_i32(&self.gl, self.color_program, "uPalette", 1);
        set_vec2(
            &self.gl,
            self.color_program,
            "uResolution",
            self.w as f32,
            self.h as f32,
        );
        set_f32(
            &self.gl,
            self.color_program,
            "uPaletteScroll",
            app.palette_scroll,
        );
        set_f32(
            &self.gl,
            self.color_program,
            "uPalettePhase",
            app.current_palette().phase as f32,
        );
        set_f32(
            &self.gl,
            self.color_program,
            "uNormMax",
            app.state.profile_norm_max(),
        );
        self.gl.draw_arrays(glow::TRIANGLES, 0, 3);
    }

    unsafe fn set_state_uniforms(&self, app: &App, program: glow::NativeProgram) {
        let s = &app.state;
        set_i32(&self.gl, program, "uFlipU", s.flip_u as i32);
        set_i32(&self.gl, program, "uFlipV", s.flip_v as i32);
        set_i32(&self.gl, program, "uPhasePerturb", s.phase_perturb as i32);
        set_i32(&self.gl, program, "uBlendUv", s.blend_uv as i32);
        set_i32(
            &self.gl,
            program,
            "uStripeRadiusBlend",
            s.stripe_radius_blend as i32,
        );
        set_i32(&self.gl, program, "uInvertFirst", s.invert_first as i32);
        set_i32(&self.gl, program, "uRhombuses", s.rhombuses as i32);
        set_i32(&self.gl, program, "uMesh", s.mesh as i32);
        set_i32(&self.gl, program, "uCoordMode", s.coord_mode);
        set_i32(&self.gl, program, "uSw1", s.sw1);
        set_i32(&self.gl, program, "uSw2", s.sw2);
        set_i32(&self.gl, program, "uSw3", s.sw3);
        set_i32(&self.gl, program, "uLateSel", s.late_sel);
        set_f32(&self.gl, program, "uF3ecc", s.f3ecc);
        set_f32(&self.gl, program, "uF3ed0", s.f3ed0);
        set_f32(&self.gl, program, "uF3ed4", s.f3ed4);
        set_f32(&self.gl, program, "uF3ed8", s.f3ed8);
        set_f32(&self.gl, program, "uF3edc", s.f3edc);
        set_f32(&self.gl, program, "uF3ee0", s.f3ee0);
        set_f32(&self.gl, program, "uF3ee4", s.f3ee4);
        set_f32(&self.gl, program, "uF3ee8", s.f3ee8);
        set_f32(&self.gl, program, "uF3ef4", s.f3ef4);
        set_f32(&self.gl, program, "uF3ef8", s.f3ef8);
        set_f32(&self.gl, program, "uF3efc", s.f3efc);
        set_f32(&self.gl, program, "uF3f00", s.f3f00);
        set_f32(&self.gl, program, "uF3f04", s.f3f04);
        set_f32(&self.gl, program, "uF3f08", s.f3f08);
        set_f32(&self.gl, program, "uF3f0c", s.f3f0c);
        set_f32(&self.gl, program, "uF3f10", s.f3f10);
        set_f32(&self.gl, program, "uF3f14", s.f3f14);
        set_f32(&self.gl, program, "uF3f18", s.f3f18);
        set_f32(&self.gl, program, "uF3f1c", s.f3f1c);
        set_f32(&self.gl, program, "uF3f20", s.f3f20);
        set_f32(&self.gl, program, "uF3f24", s.f3f24);
        set_f32(&self.gl, program, "uF3f28", s.f3f28);
        set_f32(&self.gl, program, "uF3f2c", s.f3f2c);
        set_f32(&self.gl, program, "uF3f30", s.f3f30);
        set_f32(&self.gl, program, "uF3f34", s.f3f34);
        set_f32(&self.gl, program, "uF3f38", s.f3f38);
        set_f32(&self.gl, program, "uF3f3c", s.f3f3c);
        set_f32(&self.gl, program, "uF3f40", s.f3f40);
        set_f32(&self.gl, program, "uF3f44", s.f3f44);
        set_f32(&self.gl, program, "uF3f48", s.f3f48);
        set_f32(&self.gl, program, "uF3f4c", s.f3f4c);
        set_f32(&self.gl, program, "uF3f50", s.f3f50);
        set_f32(&self.gl, program, "uF3f54", s.f3f54);
        set_f32(&self.gl, program, "uF3f58", s.f3f58);
        set_f32(&self.gl, program, "uF3f5c", s.f3f5c);
        set_f32(&self.gl, program, "uF3f60", s.f3f60);
        set_f32(&self.gl, program, "uF3f64", s.f3f64);
        set_f32(&self.gl, program, "uF3f70", s.f3f70);
        set_vec4(&self.gl, program, "uQ0", s.q[0], s.q[1], s.q[2], s.q[3]);
        set_vec4(&self.gl, program, "uQ1", s.q[4], s.q[5], s.q[6], s.q[7]);
    }
}

unsafe fn compile_program(
    gl: &glow::Context,
    vertex_src: &str,
    fragment_src: &str,
) -> Result<glow::NativeProgram, String> {
    let program = gl.create_program()?;
    let vs = compile_shader(gl, glow::VERTEX_SHADER, vertex_src)?;
    let fs = compile_shader(gl, glow::FRAGMENT_SHADER, fragment_src)?;
    gl.attach_shader(program, vs);
    gl.attach_shader(program, fs);
    gl.link_program(program);
    if !gl.get_program_link_status(program) {
        let log = gl.get_program_info_log(program);
        gl.delete_shader(vs);
        gl.delete_shader(fs);
        gl.delete_program(program);
        return Err(log);
    }
    gl.detach_shader(program, vs);
    gl.detach_shader(program, fs);
    gl.delete_shader(vs);
    gl.delete_shader(fs);
    Ok(program)
}

unsafe fn compile_shader(
    gl: &glow::Context,
    kind: u32,
    source: &str,
) -> Result<glow::NativeShader, String> {
    let shader = gl.create_shader(kind)?;
    gl.shader_source(shader, source);
    gl.compile_shader(shader);
    if !gl.get_shader_compile_status(shader) {
        let log = gl.get_shader_info_log(shader);
        gl.delete_shader(shader);
        Err(log)
    } else {
        Ok(shader)
    }
}

unsafe fn set_i32(gl: &glow::Context, program: glow::NativeProgram, name: &str, value: i32) {
    let loc = gl.get_uniform_location(program, name);
    gl.uniform_1_i32(loc.as_ref(), value);
}
unsafe fn set_f32(gl: &glow::Context, program: glow::NativeProgram, name: &str, value: f32) {
    let loc = gl.get_uniform_location(program, name);
    gl.uniform_1_f32(loc.as_ref(), value);
}
unsafe fn set_vec2(gl: &glow::Context, program: glow::NativeProgram, name: &str, x: f32, y: f32) {
    let loc = gl.get_uniform_location(program, name);
    gl.uniform_2_f32(loc.as_ref(), x, y);
}
unsafe fn set_vec4(
    gl: &glow::Context,
    program: glow::NativeProgram,
    name: &str,
    x: f32,
    y: f32,
    z: f32,
    w: f32,
) {
    let loc = gl.get_uniform_location(program, name);
    gl.uniform_4_f32(loc.as_ref(), x, y, z, w);
}

struct App {
    w: usize,
    h: usize,
    sin_lut: Vec<f32>,
    cos_lut: Vec<f32>,
    palettes: Vec<Palette>,
    pal_idx: usize,
    palette_scroll: f32,
    state: State,
    patterns: Vec<State>,
    pattern_idx: usize,
    scalar: Vec<f32>,
    frame: Vec<u32>,
    min_v: f32,
    max_v: f32,
    paused: bool,
}

impl App {
    fn new(w: usize, h: usize) -> Self {
        let mut sin_lut = vec![0.0; LUT_N];
        let mut cos_lut = vec![0.0; LUT_N];
        for i in 0..LUT_N {
            let a = 2.0 * PI * i as f32 / LUT_N as f32;
            sin_lut[i] = a.sin();
            cos_lut[i] = a.cos();
        }
        let palettes = build_palettes();
        let pal_idx = random_index(palettes.len());
        let mut app = Self {
            w,
            h,
            sin_lut,
            cos_lut,
            palettes,
            pal_idx,
            palette_scroll: 0.0,
            state: State::new(seed_now()),
            patterns: Vec::new(),
            pattern_idx: 0,
            scalar: vec![0.0; w * h],
            frame: vec![0; w * h],
            min_v: 0.0,
            max_v: 1.0,
            paused: false,
        };
        app.new_pattern();
        app
    }

    fn new_pattern(&mut self) {
        if self.pattern_idx + 1 < self.patterns.len() {
            self.patterns.truncate(self.pattern_idx + 1);
        }
        self.state = State::new(seed_now());
        self.state.build();
        self.state.build_profile();
        self.patterns.push(self.state.clone());
        self.pattern_idx = self.patterns.len() - 1;
        self.log_pattern("pattern");
    }

    fn next_pattern(&mut self) {
        if self.pattern_idx + 1 < self.patterns.len() {
            self.pattern_idx += 1;
            self.state = self.patterns[self.pattern_idx].clone();
            self.log_pattern("pattern next");
        } else {
            self.new_pattern();
        }
    }

    fn prev_pattern(&mut self) {
        if self.pattern_idx > 0 {
            self.pattern_idx -= 1;
            self.state = self.patterns[self.pattern_idx].clone();
            self.log_pattern("pattern prev");
        } else {
            eprintln!("pattern: already at oldest pattern");
        }
    }

    fn log_pattern(&self, label: &str) {
        eprintln!(
            "{}: {}/{} style={} palette={}",
            label,
            self.pattern_idx + 1,
            self.patterns.len(),
            self.state.style_name(),
            self.palettes[self.pal_idx].name
        );
    }

    fn resize(&mut self, w: usize, h: usize) -> bool {
        let w = w.max(1);
        let h = h.max(1);
        if self.w == w && self.h == h {
            return false;
        }
        self.w = w;
        self.h = h;
        true
    }

    fn current_palette(&self) -> &Palette {
        &self.palettes[self.pal_idx]
    }

    fn prev_palette(&mut self) {
        self.pal_idx = (self.pal_idx + self.palettes.len() - 1) % self.palettes.len();
        self.palette_scroll = 0.0;
        eprintln!("palette: {}", self.palettes[self.pal_idx].name);
    }

    fn next_palette(&mut self) {
        self.pal_idx = (self.pal_idx + 1) % self.palettes.len();
        self.palette_scroll = 0.0;
        eprintln!("palette: {}", self.palettes[self.pal_idx].name);
    }

    fn random_palette(&mut self) {
        let len = self.palettes.len();
        if len > 1 {
            let mut idx = random_index(len - 1);
            if idx >= self.pal_idx {
                idx += 1;
            }
            self.pal_idx = idx;
        }
        self.palette_scroll = 0.0;
        eprintln!("palette random: {}", self.palettes[self.pal_idx].name);
    }

    fn li(&self, phase: f32) -> usize {
        (phase.round() as i32 as u32 & 0xffff) as usize
    }
    fn sl(&self, phase: f32) -> f32 {
        self.sin_lut[self.li(phase)]
    }
    fn cl(&self, phase: f32) -> f32 {
        self.cos_lut[self.li(phase)]
    }

    fn render_scalar(&mut self) {
        self.min_v = f32::INFINITY;
        self.max_v = f32::NEG_INFINITY;
        let hw = self.w as f32 * 0.5;
        let hh = self.h as f32 * 0.5;
        for y in 0..self.h {
            for x in 0..self.w {
                let val = self.sample_scalar_at(x as f32 + 0.5, y as f32 + 0.5, hw, hh);
                let idx = y * self.w + x;
                self.scalar[idx] = val;
                self.min_v = self.min_v.min(val);
                self.max_v = self.max_v.max(val);
            }
        }
        if self.max_v <= self.min_v {
            self.max_v = self.min_v + 1.0;
        }
    }

    fn sample_scalar_at(&self, px: f32, py: f32, hw: f32, hh: f32) -> f32 {
        let mut u = (px - hw) / hw.max(1.0);
        let mut v = (py - hh) / hh.max(1.0);
        let s = &self.state;
        if s.flip_u {
            u = -u;
        }
        if s.flip_v {
            v = -v;
        }
        if s.phase_perturb {
            v = (PI * v * s.f3f64 + s.f3f70).cos();
        }
        let raw_v = v;
        if s.blend_uv {
            u = 0.5 * (u + v);
        }

        let mut angle = u.atan2(v).abs() / PI;
        let mut radius = sat(INV_SQRT2 * (0.45 + 0.9999 * (u * u + v * v).sqrt()));

        if s.f3f60 != 0.0 && s.sw1 != 8 {
            let mut frac = (s.f3f60 * angle).fract();
            if frac < 0.0 {
                frac += 1.0;
            }
            let stripe = 2.0 * (frac - 0.5).abs();
            angle = stripe;
            if s.stripe_radius_blend && s.f3f60 < 5.0 && s.sw2 != 9 && s.sw3 != 9 && !s.blend_uv {
                radius = 0.5 * (radius + stripe);
            }
        }

        if s.coord_mode == 1 {
            let exp = (self.sl((radius + s.f3ed0) * 32768.0) + s.f3ed0)
                .round()
                .max(1.0);
            radius = sat(safe_pow(radius.abs() + 0.45, exp));
        } else if s.coord_mode == 2 {
            let (uu, vv) = s.apply_quadratic(u, raw_v);
            u = uu;
            v = vv;
            radius = sat(0.45 + 0.55 * uu.abs());
            angle = sat(0.5 + 0.5 * vv);
        } else if s.coord_mode == 3 {
            radius = sat(0.5 + 0.5 * self.sl((u + v + s.f3ed4) * 32768.0));
        }

        let mut s1 = self.first_switch(radius, angle, u, v);
        if s.invert_first {
            s1 = 1.0 - s1;
        }
        s1 = sat(s1);
        let s2 = self.second_switch(s1, angle, u, v);
        let (aux_a, aux_b) = if s.late_sel == 0 {
            ((1.0 - s.f3ecc) + s.f3ecc * s2, s.f3ecc * s2)
        } else {
            (1.0, 0.0)
        };
        let s3 = self.third_switch(s1, s2, aux_a, aux_b, angle, u, v);
        let mut coord = radius * PROFILE_SCALE;
        if self.style_gate(s1, angle) {
            coord = self.late_combine(s2, s3);
        }
        coord = coord.clamp(0.0, PROFILE_SCALE);
        self.state.sample_profile(coord) * s1.max(0.001)
    }

    fn first_switch(&self, s1: f32, a: f32, u: f32, v: f32) -> f32 {
        let s = &self.state;
        match s.sw1 {
            0 => s1,
            1 => {
                s1 / (1.0
                    + self
                        .sl((s.f3f48 / (s1 + 0.1) + s.f3f4c * PI * a) * 32768.0)
                        .abs())
            }
            2 => {
                s1 / (1.0
                    + self
                        .sl((2.0 * PI * s.f3f48 / (s1 + 0.1) + s.f3f4c * PI * a) * 32768.0)
                        .abs())
            }
            3 => {
                s1 / (1.0
                    + (self.sl(s.f3f50 * s1 * 32768.0) * self.cl(s.f3f54 * a * 32768.0)).abs())
            }
            4 => s1 / (1.0 + self.cl(s.f3f48 * s1 * a * 65536.0).abs()),
            5 => s1 / (1.0 + self.sl((1.0 + s1).ln() * s.f3f48 * 32768.0).abs()),
            6 => s1 * ((1.0 - s.f3f58) + self.sl(s.f3f5c * a * 32768.0).abs() * s.f3f58),
            7 => s1 / (2.0 + self.cl(65536.0 * u * v)),
            8 => s1 / (2.0 + self.sl(65536.0 * u * v)),
            _ => s1,
        }
    }

    fn second_switch(&self, s1: f32, a: f32, u: f32, v: f32) -> f32 {
        let s = &self.state;
        sat(match s.sw2 {
            0 => safe_pow(
                u.abs() + 0.0001,
                (self.sl(s.f3efc * a) * s.f3f00 + s.f3f08 + self.sl((s.f3f04 + v) * 32768.0)).abs(),
            ),
            1 => {
                1.0 / (2.0
                    + self.sl(s.f3ee4 * s1 * 32768.0) * (1.0 - s1) * self.sl(s.f3ee8 * a * 32768.0))
            }
            2 => self.cl(s.f3ed8 * a * 32768.0).abs() / (1.0 + (s.f3edc * s1).abs()),
            3 => {
                1.0 / (2.0 + self.sl(s.f3ee4 * s1 * 32768.0) * s1 * self.sl(s.f3ee8 * a * 32768.0))
            }
            4 => {
                1.0 / (1.0
                    + self
                        .sl((0.02 + (u * v).abs() + s1).ln() * s.f3ee0 * 32768.0)
                        .abs())
            }
            5 => (1.0 - s.f3f58) + s.f3f58 * self.sl(s.f3efc * a + s.f3ef8 * 32768.0).abs(),
            6 => 1.0 / (2.0 + self.cl(65536.0 * u * v * s.f3ef4)),
            7 => 1.0 / (2.0 + self.sl(65536.0 * u * v * s.f3ef8)),
            8 => ((1.2 + 0.2 * s1) / (1.0 + (u * v).abs())).abs(),
            9 => ((s1 + 0.05) / s.f3f40.abs().max(0.001) + a).cos().abs(),
            _ => s1,
        })
    }

    fn third_switch(
        &self,
        s1: f32,
        s2: f32,
        aux_a: f32,
        _aux_b: f32,
        a: f32,
        u: f32,
        v: f32,
    ) -> f32 {
        let s = &self.state;
        sat(match s.sw3 {
            0 => safe_pow(
                u.abs() + 0.0001,
                (self.sl(s.f3f30 * a) * s.f3f34 + s.f3f3c + self.sl((s.f3f38 + v) * 32768.0)).abs(),
            ),
            1 => {
                1.0 / (2.0
                    + self.sl(s.f3f18 * s1 * 32768.0) * aux_a * self.sl(s.f3f1c * a * 32768.0))
            }
            2 => self.cl(s.f3f0c * a * 32768.0).abs() / (1.0 + (s.f3f10 * aux_a).abs()),
            3 => {
                1.0 / (2.0
                    + self.sl(s.f3f18 * s1 * 32768.0)
                        * aux_a
                        * self.sl(s.f3f1c * a * 32768.0)
                        * s2.max(0.1))
            }
            4 => {
                1.0 / (1.0
                    + self
                        .sl((0.02 + (aux_a * u * v).abs() + s1).ln() * s.f3f14 * 32768.0)
                        .abs())
            }
            5 => {
                (1.0 - s.f3f58) + s.f3f58 * self.sl(s.f3f28 * a * 32768.0 + s.f3f2c * 8192.0).abs()
            }
            6 => 1.0 / (1.2 + 0.2 * self.cl(s.f3f20 * u * v * 65536.0).abs()),
            7 => 1.0 / (5.0 + 0.5 * self.sl(s.f3f24 * u * v * 65536.0).abs()),
            8 => ((s1 + 0.05) / s.f3f44.abs().max(0.001) + a).cos().abs(),
            9 => s2,
            _ => s2,
        })
    }

    fn style_gate(&self, s1: f32, a: f32) -> bool {
        let s = &self.state;
        if !s.rhombuses && !s.mesh {
            return true;
        }
        if s.rhombuses {
            let aa = self.sl(100.0 * a * 32768.0);
            let bb = self.sl(50.0 * s1 * s1 * 32768.0);
            return bb > aa;
        }
        let aa = self.sl(100.0 * a * 32768.0);
        let bb = self.sl(50.0 * s1 * 32768.0);
        (aa - bb).abs() >= 0.25
    }

    fn late_combine(&self, s2: f32, s3: f32) -> f32 {
        let e = self.state.f3ecc;
        match self.state.late_sel {
            0 => PROFILE_SCALE * s3,
            1 => PROFILE_SCALE * (e * s2 + (1.0 - e) * s3),
            2 => PROFILE_SCALE / (1.0 + 30.0 * (e * s2 + (1.0 - e) * s3)),
            3 => PROFILE_SCALE / (1.0 + 10.0 * e * s2 * s3),
            4 => (PROFILE_SCALE * s2) / (1.0 + 10.0 * e * s3),
            5 => PROFILE_SCALE * safe_pow(s2, (1.0 + e) * s3),
            _ => PROFILE_SCALE * s3,
        }
    }

    fn colorize(&mut self) {
        let pal = &self.palettes[self.pal_idx];
        let inv = 1.0 / (self.max_v - self.min_v);
        let edge_threshold = (self.max_v - self.min_v) * 0.018;
        for y in 0..self.h {
            for x in 0..self.w {
                let i = y * self.w + x;
                let center = self.scalar[i];
                let color = if self.scalar_edge_strength(x, y) > edge_threshold {
                    let mut r = 0u32;
                    let mut g = 0u32;
                    let mut b = 0u32;
                    for &(ox, oy) in POST_AA_OFFSETS {
                        let value = sample_scalar_bilinear(
                            &self.scalar,
                            self.w,
                            self.h,
                            x as f32 + ox,
                            y as f32 + oy,
                        );
                        let c = color_for_scalar(value, self.min_v, inv, self.palette_scroll, pal);
                        r += (c >> 16) & 255;
                        g += (c >> 8) & 255;
                        b += c & 255;
                    }
                    rgb((r / 4) as i32, (g / 4) as i32, (b / 4) as i32)
                } else {
                    color_for_scalar(center, self.min_v, inv, self.palette_scroll, pal)
                };
                self.frame[i] = color;
            }
        }
    }

    fn scalar_edge_strength(&self, x: usize, y: usize) -> f32 {
        let i = y * self.w + x;
        let c = self.scalar[i];
        let xl = x.saturating_sub(1);
        let xr = (x + 1).min(self.w - 1);
        let yu = y.saturating_sub(1);
        let yd = (y + 1).min(self.h - 1);
        let dx = (self.scalar[y * self.w + xr] - self.scalar[y * self.w + xl]).abs();
        let dy = (self.scalar[yd * self.w + x] - self.scalar[yu * self.w + x]).abs();
        let diag_a = (self.scalar[yd * self.w + xr] - self.scalar[yu * self.w + xl]).abs();
        let diag_b = (self.scalar[yd * self.w + xl] - self.scalar[yu * self.w + xr]).abs();
        dx.max(dy)
            .max(diag_a)
            .max(diag_b)
            .max((c - self.scalar[y * self.w + xl]).abs())
    }
}

#[derive(Clone)]
struct State {
    rng: Rng,
    profile: Vec<f32>,
    h_amp: [f32; 4],
    h_phase: [f32; 4],
    ribbons: bool,
    rhombuses: bool,
    mesh: bool,
    flip_u: bool,
    flip_v: bool,
    phase_perturb: bool,
    blend_uv: bool,
    stripe_radius_blend: bool,
    invert_first: bool,
    coord_mode: i32,
    sw1: i32,
    sw2: i32,
    sw3: i32,
    late_sel: i32,
    f3ea4: f32,
    f3ea8: f32,
    f3eac: f32,
    f3eb0: f32,
    f3eb4: f32,
    f3fb4: f32,
    f3fb8: f32,
    f3ecc: f32,
    f3ed0: f32,
    f3ed4: f32,
    f3ed8: f32,
    f3edc: f32,
    f3ee0: f32,
    f3ee4: f32,
    f3ee8: f32,
    f3ef4: f32,
    f3ef8: f32,
    f3efc: f32,
    f3f00: f32,
    f3f04: f32,
    f3f08: f32,
    f3f0c: f32,
    f3f10: f32,
    f3f14: f32,
    f3f18: f32,
    f3f1c: f32,
    f3f20: f32,
    f3f24: f32,
    f3f28: f32,
    f3f2c: f32,
    f3f30: f32,
    f3f34: f32,
    f3f38: f32,
    f3f3c: f32,
    f3f40: f32,
    f3f44: f32,
    f3f48: f32,
    f3f4c: f32,
    f3f50: f32,
    f3f54: f32,
    f3f58: f32,
    f3f5c: f32,
    f3f60: f32,
    f3f64: f32,
    f3f68: f32,
    f3f6c: f32,
    f3f70: f32,
    f3f74: f32,
    q: [f32; 8],
}

impl State {
    fn new(seed: u64) -> Self {
        Self {
            rng: Rng::new(seed),
            profile: vec![0.0; PROFILE_N],
            h_amp: [0.0; 4],
            h_phase: [0.0; 4],
            ribbons: false,
            rhombuses: false,
            mesh: false,
            flip_u: false,
            flip_v: false,
            phase_perturb: false,
            blend_uv: false,
            stripe_radius_blend: false,
            invert_first: false,
            coord_mode: 0,
            sw1: 0,
            sw2: 0,
            sw3: 0,
            late_sel: 0,
            f3ea4: 1.0,
            f3ea8: 0.0,
            f3eac: 0.1,
            f3eb0: 0.4,
            f3eb4: 1.0,
            f3fb4: 0.5,
            f3fb8: 3.0,
            f3ecc: 0.5,
            f3ed0: 0.5,
            f3ed4: 0.0,
            f3ed8: 1.0,
            f3edc: 1.0,
            f3ee0: 1.0,
            f3ee4: 1.0,
            f3ee8: 1.0,
            f3ef4: 1.0,
            f3ef8: 1.0,
            f3efc: 32768.0,
            f3f00: 1.0,
            f3f04: 0.0,
            f3f08: 0.0,
            f3f0c: 1.0,
            f3f10: 1.0,
            f3f14: 1.0,
            f3f18: 1.0,
            f3f1c: 1.0,
            f3f20: 1.0,
            f3f24: 1.0,
            f3f28: 1.0,
            f3f2c: 1.0,
            f3f30: 32768.0,
            f3f34: 1.0,
            f3f38: 0.0,
            f3f3c: 0.0,
            f3f40: 0.5,
            f3f44: 0.5,
            f3f48: 0.5,
            f3f4c: 0.5,
            f3f50: 0.5,
            f3f54: 0.5,
            f3f58: 0.5,
            f3f5c: 1.0,
            f3f60: 0.0,
            f3f64: 0.2,
            f3f68: 0.1,
            f3f6c: 0.0,
            f3f70: PI / 2.0,
            f3f74: 0.0,
            q: [0.0; 8],
        }
    }

    fn ri(&mut self, n: u32) -> u32 {
        self.rng.range(n.max(1))
    }
    fn rb(&mut self) -> bool {
        self.rng.next_u32() & 1 != 0
    }
    fn rf(&mut self) -> f32 {
        self.rng.f32()
    }
    fn sr(&mut self) -> f32 {
        1.0 - 2.0 * (self.ri(32768) as f32 / 32768.0)
    }
    fn sign(&mut self) -> f32 {
        if self.rb() {
            1.0
        } else {
            -1.0
        }
    }
    fn wide(&mut self, minc: f32, spanc: f32) -> f32 {
        if self.rb() {
            3.0 + 0.8 * minc + 0.008 * spanc * self.ri(100) as f32
        } else {
            20.0 + 2.0 * minc + 0.02 * spanc * self.ri(100) as f32
        }
    }
    fn mid(&mut self, minc: f32, spanc: f32) -> f32 {
        2.0 + 0.4 * minc + 0.004 * spanc * self.ri(100) as f32
    }

    fn build(&mut self) {
        let minc = 2.0 + self.ri(3) as f32;
        let maxc = 8.0 + self.ri(5) as f32;
        let spanc = (maxc - minc).max(1.0);
        let style = self.ri(4);
        self.ribbons = style == 1;
        self.rhombuses = style == 2;
        self.mesh = style == 3;
        self.flip_u = self.rb();
        self.flip_v = self.rb();
        self.invert_first = self.rb();
        self.phase_perturb = self.rb();
        self.blend_uv = self.ri(7) == 0;
        self.stripe_radius_blend = self.ri(3) == 0;
        for k in 0..4 {
            self.h_amp[k] = self.sr();
            self.h_phase[k] = PI * self.sr();
        }
        self.f3eac = 0.01 + 0.05 * (2.0 + 7.0 * (0.01 * self.ri(100) as f32));
        self.f3ea4 = (1.0 + minc + 0.01 * spanc * self.ri(100) as f32).round();
        self.f3ea8 = if self.sr() > 0.0 { 1.0 } else { 0.0 };
        self.f3eb0 = if self.rb() { 4.0 } else { 0.4 };
        self.f3eb4 = 0.85 + 0.5 * self.rf();
        self.f3fb4 = if self.rb() {
            0.2 + 0.005 * self.ri(100) as f32
        } else {
            1.3 + 0.005 * self.ri(100) as f32
        };
        self.f3fb8 = self.mid(minc, spanc);
        self.f3ecc = 0.01 * self.ri(100) as f32;
        self.f3ed0 = 0.5 + 0.02 * self.ri(100) as f32;
        self.f3ed4 = 0.01 * self.ri(100) as f32;
        self.f3ed8 = 1.0 + 0.5 * minc + 0.005 * spanc * self.ri(100) as f32;
        self.f3f0c = 1.0 + 0.5 * minc + 0.005 * spanc * self.ri(100) as f32;
        self.f3edc = minc + 0.01 * spanc * self.ri(100) as f32;
        self.f3f10 = minc + 0.01 * spanc * self.ri(100) as f32;
        self.f3ee0 = 0.1 + 0.39 * minc + 0.0039 * spanc * self.ri(100) as f32;
        self.f3f14 = 0.1 + 0.39 * minc + 0.0039 * spanc * self.ri(100) as f32;
        self.f3ee4 = self.wide(minc, spanc);
        self.f3f18 = self.wide(minc, spanc);
        self.f3ee8 = self.wide(minc, spanc);
        self.f3f1c = self.wide(minc, spanc);
        self.f3ef4 = 1.0 + 2.0 * minc + 0.02 * spanc * self.ri(100) as f32;
        self.f3f28 = 1.0 + 2.0 * minc + 0.02 * spanc * self.ri(100) as f32;
        self.f3ef8 = minc + self.ri((2.0 * spanc).round().max(1.0) as u32) as f32;
        self.f3f2c = minc + self.ri((2.0 * spanc).round().max(1.0) as u32) as f32;
        self.f3efc = (5 + self.ri(20)) as f32 * 32768.0;
        self.f3f30 = (5 + self.ri(20)) as f32 * 32768.0;
        self.f3f00 = 1.0 + 0.07 * self.ri(100) as f32;
        self.f3f34 = 1.0 + 0.07 * self.ri(100) as f32;
        self.f3f04 = 0.005 * self.ri(100) as f32;
        self.f3f38 = 0.005 * self.ri(100) as f32;
        self.f3f08 = 0.04 * self.ri(100) as f32;
        self.f3f3c = 0.04 * self.ri(100) as f32;
        self.f3f40 = self.sign() * (0.2 + 0.008 * self.ri(100) as f32);
        self.f3f44 = self.sign() * (0.2 + 0.008 * self.ri(100) as f32);
        self.f3f48 = 0.1 * minc + 0.001 * spanc * self.ri(100) as f32;
        self.f3f4c = 0.1 * minc + 0.001 * spanc * self.ri(100) as f32;
        self.f3f50 = 0.2 + 0.03 * minc + 0.0003 * spanc * self.ri(100) as f32;
        self.f3f54 = 0.05 * minc + 0.0005 * spanc * self.ri(100) as f32;
        self.f3f58 = 0.5 + 0.005 * self.ri(100) as f32;
        self.f3f5c = [0.8, 1.0, 1.5, 2.0][self.ri(4) as usize];
        self.f3f60 = if self.rb() {
            0.0
        } else {
            0.5 * self.ri((2.0 * spanc).round().max(1.0) as u32) as f32
        };
        self.f3f64 = 0.15 + 0.0015 * self.ri(100) as f32;
        self.f3f68 = if self.rb() {
            0.25
        } else {
            0.1 + 0.0015 * self.ri(100) as f32
        };
        self.f3f6c = -1.0 + 0.02 * self.ri(100) as f32;
        self.f3f70 = ((1.0 - 0.02 * self.ri(100) as f32) * (PI / 4.0)) + PI / 2.0;
        self.f3f74 = 0.005 * self.ri(100) as f32;
        self.coord_mode = self.ri(4) as i32;
        self.sw1 = self.ri(9) as i32;
        self.sw2 = self.ri(10) as i32;
        self.sw3 = self.ri(10) as i32;
        self.late_sel = self.ri(6) as i32;
        self.q = match self.ri(6) {
            0 => [0.0, 0.0, 1.0, 0.0, -1.0, 0.0, 1.0, 0.0],
            1 => [0.0, 0.0, -1.0, 0.0, 1.0, 1.0, 0.0, -1.0],
            2 => [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
            3 => [0.0, 0.0, 1.0, -1.0, 0.0, 0.0, 1.0, -1.0],
            4 => [0.0, 0.0, -0.5, 1.0, 0.5, 0.5, -1.0, 0.5],
            _ => [0.0, 0.0, 1.0, 1.0, -1.0, -1.0, 1.0, 1.0],
        };
    }

    fn build_profile(&mut self) {
        let mut mn = f32::INFINITY;
        let mut mx = f32::NEG_INFINITY;
        for i in 0..PROFILE_N {
            let phi = i as f32 * (PI / 2000.0) - PI;
            let raw = if self.ribbons {
                if (PI * self.f3fb8 * phi).sin() + 1.0 < self.f3fb4 {
                    4000.0
                } else {
                    0.0
                }
            } else {
                let mut sum = 0.0;
                for k in 0..4 {
                    let amp = self.h_amp[k] * safe_pow((k + 1) as f32, -self.f3eb0);
                    let theta = self.f3ea8 * self.h_phase[k] + (self.f3ea4 * (k + 1) as f32) * phi;
                    sum += amp * theta.cos();
                }
                4000.0 * sum
            };
            self.profile[i] = raw;
            mn = mn.min(raw);
            mx = mx.max(raw);
        }
        let inv = 1.0 / (mx - mn).max(0.0001);
        for v in &mut self.profile {
            let n = (*v - mn) * inv;
            *v = self.f3eb4 * (4000.0 + (8000.0 * self.f3eac) * n);
        }
    }

    fn apply_quadratic(&self, u: f32, v: f32) -> (f32, f32) {
        let up =
            self.q[0] + 0.5 * (self.q[2] * v * v + 2.0 * self.q[3] * u * v + self.q[4] * u * u);
        let vp =
            self.q[1] + 0.5 * (self.q[5] * v * v + 2.0 * self.q[6] * u * v + self.q[7] * u * u);
        (up.clamp(-1.0, 1.0), vp.clamp(-1.0, 1.0))
    }

    fn sample_profile(&self, coord: f32) -> f32 {
        let i = (coord.round() as isize).clamp(0, PROFILE_N as isize - 2) as usize;
        if self.ribbons {
            self.profile[i]
        } else {
            let t = (coord - i as f32).clamp(0.0, 1.0);
            self.profile[i] * (1.0 - t) + self.profile[i + 1] * t
        }
    }

    fn style_name(&self) -> &'static str {
        if self.ribbons {
            "Ribbons"
        } else if self.rhombuses {
            "Rhombuses"
        } else if self.mesh {
            "Mesh"
        } else {
            "Embossing"
        }
    }

    fn profile_norm_max(&self) -> f32 {
        self.profile
            .iter()
            .copied()
            .fold(1.0_f32, f32::max)
            .max(1.0)
            * 0.85
    }
}

#[derive(Clone)]
struct Rng(u64);
impl Rng {
    fn new(seed: u64) -> Self {
        Self(seed | 1)
    }
    fn next_u32(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 32) as u32
    }
    fn range(&mut self, n: u32) -> u32 {
        self.next_u32() % n
    }
    fn f32(&mut self) -> f32 {
        self.next_u32() as f32 / u32::MAX as f32
    }
}

#[derive(Clone)]
struct Node {
    pos: i32,
    r: i32,
    g: i32,
    b: i32,
}

#[derive(Deserialize)]
struct PaletteJson {
    name: String,
    phase: i32,
    nodes: Vec<[i32; 4]>,
}

struct Palette {
    name: String,
    phase: i32,
    nodes: Vec<Node>,
    dense: Vec<u32>,
}

fn build_palettes() -> Vec<Palette> {
    let mut palettes = parse_palettes_json(PALETTES_JSON).expect("parse embedded palettes.json");
    for pal in &mut palettes {
        build_dense_palette(pal);
    }
    eprintln!("loaded {} palettes", palettes.len());
    palettes
}

fn parse_palettes_json(src: &str) -> Result<Vec<Palette>, String> {
    let palettes: Vec<PaletteJson> = serde_json::from_str(src)
        .map_err(|err| format!("failed to parse embedded palettes.json: {err}"))?;
    if palettes.is_empty() {
        return Err("no palettes found".to_owned());
    }

    palettes
        .into_iter()
        .map(|p| {
            if p.nodes.is_empty() {
                return Err(format!("palette {} has no nodes", p.name));
            }
            let nodes = p
                .nodes
                .into_iter()
                .map(|n| Node {
                    pos: n[0],
                    r: n[1],
                    g: n[2],
                    b: n[3],
                })
                .collect();
            Ok(Palette {
                name: p.name,
                phase: p.phase,
                nodes,
                dense: vec![0; PAL_N],
            })
        })
        .collect()
}

fn build_dense_palette(p: &mut Palette) {
    p.nodes.sort_by(|a, b| a.pos.cmp(&b.pos));
    for ph in 0..PAL_N {
        let r = sample_chan(p, ph as i32, 0);
        let g = sample_chan(p, ph as i32, 1);
        let b = sample_chan(p, ph as i32, 2);
        p.dense[ph] = rgb(r, g, b);
    }
}

fn sample_chan(p: &Palette, phase: i32, ch: usize) -> i32 {
    let n = p.nodes.len();
    let mut seg = 0usize;
    let mut pp = phase as f32;
    for i in 0..n {
        let j = (i + 1) % n;
        let x0 = p.nodes[i].pos;
        let mut x1 = p.nodes[j].pos;
        if x1 <= x0 {
            x1 += 1024;
        }
        let mut q = phase;
        if q < x0 {
            q += 1024;
        }
        if q >= x0 && q < x1 {
            seg = i;
            pp = q as f32;
            break;
        }
    }
    let prev = &p.nodes[(seg + n - 1) % n];
    let cur = &p.nodes[seg];
    let next = &p.nodes[(seg + 1) % n];
    let nn = &p.nodes[(seg + 2) % n];
    let mut xp = prev.pos as f32;
    let x0 = cur.pos as f32;
    let mut x1 = next.pos as f32;
    let mut x2 = nn.pos as f32;
    if xp > x0 {
        xp -= 1024.0;
    }
    if x1 <= x0 {
        x1 += 1024.0;
    }
    while x2 <= x1 {
        x2 += 1024.0;
    }
    let yp = chan(prev, ch) as f32;
    let y0 = chan(cur, ch) as f32;
    let y1 = chan(next, ch) as f32;
    let y2 = chan(nn, ch) as f32;
    let m0 = (y0 - yp) / (x0 - xp).abs().max(1.0);
    let m1 = (y1 - y0) / (x1 - x0).abs().max(1.0);
    let m2 = (y2 - y1) / (x2 - x1).abs().max(1.0);
    let a = 0.5 * ((4.0 * ((m0 - m1).abs() - 0.8)).tanh() + 1.0);
    let b = 0.5 * ((4.0 * ((m1 - m2).abs() - 0.8)).tanh() + 1.0);
    let d0 = (1.0 - a) * 0.5 * (m0 + m1) + a * m1;
    let d1 = (1.0 - b) * 0.5 * (m1 + m2) + b * m1;
    let dx = (x1 - x0).max(1.0);
    let s = pp - x0;
    let aa = (d0 + d1 - 2.0 * (y1 - y0) / dx) / (dx * dx);
    let bb = 3.0 * (y1 - y0) / (dx * dx) - (2.0 * d0 + d1) / dx;
    (aa * s * s * s + bb * s * s + d0 * s + y0)
        .round()
        .clamp(0.0, 255.0) as i32
}

fn chan(n: &Node, ch: usize) -> i32 {
    match ch {
        0 => n.r,
        1 => n.g,
        _ => n.b,
    }
}
fn rgb(r: i32, g: i32, b: i32) -> u32 {
    0xff000000 | ((r as u32 & 255) << 16) | ((g as u32 & 255) << 8) | (b as u32 & 255)
}
fn color_for_scalar(
    value: f32,
    min_v: f32,
    inv_range: f32,
    palette_scroll: f32,
    pal: &Palette,
) -> u32 {
    let n = ((value - min_v) * inv_range).clamp(0.0, 1.0).powf(0.82);
    let idx = ((n * 1023.0 + palette_scroll - pal.phase as f32) as i32 as u32 & 0x3ff) as usize;
    pal.dense[idx]
}
fn sample_scalar_bilinear(scalar: &[f32], w: usize, h: usize, x: f32, y: f32) -> f32 {
    let x = x.clamp(0.0, (w - 1) as f32);
    let y = y.clamp(0.0, (h - 1) as f32);
    let x0 = x.floor() as usize;
    let y0 = y.floor() as usize;
    let x1 = (x0 + 1).min(w - 1);
    let y1 = (y0 + 1).min(h - 1);
    let tx = x - x0 as f32;
    let ty = y - y0 as f32;
    let a = scalar[y0 * w + x0];
    let b = scalar[y0 * w + x1];
    let c = scalar[y1 * w + x0];
    let d = scalar[y1 * w + x1];
    let top = a * (1.0 - tx) + b * tx;
    let bottom = c * (1.0 - tx) + d * tx;
    top * (1.0 - ty) + bottom * ty
}
fn sat(x: f32) -> f32 {
    if x.is_finite() {
        x.clamp(0.0, 1.0)
    } else {
        0.0
    }
}
fn safe_pow(a: f32, b: f32) -> f32 {
    let v = a.abs().max(0.0001).powf(b.clamp(-12.0, 12.0));
    if v.is_finite() {
        v
    } else {
        0.0
    }
}
fn seed_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::from_secs(1))
        .as_nanos() as u64
}

fn random_index(len: usize) -> usize {
    if len == 0 {
        0
    } else {
        (seed_now() as usize) % len
    }
}

fn drawable_size(window: &sdl2::video::Window) -> (usize, usize) {
    let (w, h) = window.drawable_size();
    (w.max(1) as usize, h.max(1) as usize)
}

fn is_shift_down(keymod: Mod) -> bool {
    keymod.intersects(Mod::LSHIFTMOD | Mod::RSHIFTMOD)
}

const FULLSCREEN_VS: &str = r#"#version 330 core
const vec2 POS[3] = vec2[3](vec2(-1.0, -1.0), vec2(3.0, -1.0), vec2(-1.0, 3.0));
void main() {
    gl_Position = vec4(POS[gl_VertexID], 0.0, 1.0);
}
"#;

const SCALAR_FS: &str = r#"#version 330 core
layout(location = 0) out float outScalar;

uniform sampler2D uProfile;
uniform vec2 uResolution;

uniform int uFlipU;
uniform int uFlipV;
uniform int uPhasePerturb;
uniform int uBlendUv;
uniform int uStripeRadiusBlend;
uniform int uInvertFirst;
uniform int uRhombuses;
uniform int uMesh;
uniform int uCoordMode;
uniform int uSw1;
uniform int uSw2;
uniform int uSw3;
uniform int uLateSel;

uniform float uF3ecc;
uniform float uF3ed0;
uniform float uF3ed4;
uniform float uF3ed8;
uniform float uF3edc;
uniform float uF3ee0;
uniform float uF3ee4;
uniform float uF3ee8;
uniform float uF3ef4;
uniform float uF3ef8;
uniform float uF3efc;
uniform float uF3f00;
uniform float uF3f04;
uniform float uF3f08;
uniform float uF3f0c;
uniform float uF3f10;
uniform float uF3f14;
uniform float uF3f18;
uniform float uF3f1c;
uniform float uF3f20;
uniform float uF3f24;
uniform float uF3f28;
uniform float uF3f2c;
uniform float uF3f30;
uniform float uF3f34;
uniform float uF3f38;
uniform float uF3f3c;
uniform float uF3f40;
uniform float uF3f44;
uniform float uF3f48;
uniform float uF3f4c;
uniform float uF3f50;
uniform float uF3f54;
uniform float uF3f58;
uniform float uF3f5c;
uniform float uF3f60;
uniform float uF3f64;
uniform float uF3f70;
uniform vec4 uQ0;
uniform vec4 uQ1;

const float PI = 3.14159265358979323846;
const float TAU = 6.28318530717958647692;
const float INV_SQRT2 = 0.70710678118;
const float PROFILE_SCALE = 3998.0;
const float PROFILE_N = 4000.0;

float sat(float x) { return clamp(isnan(x) || isinf(x) ? 0.0 : x, 0.0, 1.0); }
float sl(float phase) { return sin(TAU * fract(phase / 65536.0)); }
float cl(float phase) { return cos(TAU * fract(phase / 65536.0)); }
float safePow(float a, float b) { return pow(max(abs(a), 0.0001), clamp(b, -12.0, 12.0)); }

float sampleProfile(float coord) {
    float u = (clamp(coord, 0.0, PROFILE_SCALE) + 0.5) / PROFILE_N;
    return texture(uProfile, vec2(u, 0.5)).r;
}

vec2 applyQuadratic(float u, float v) {
    float up = uQ0.x + 0.5 * (uQ0.z * v * v + 2.0 * uQ0.w * u * v + uQ1.x * u * u);
    float vp = uQ0.y + 0.5 * (uQ1.y * v * v + 2.0 * uQ1.z * u * v + uQ1.w * u * u);
    return clamp(vec2(up, vp), vec2(-1.0), vec2(1.0));
}

float firstSwitch(float s1, float a, float u, float v) {
    if (uSw1 == 0) return s1;
    if (uSw1 == 1) return s1 / (1.0 + abs(sl((uF3f48 / (s1 + 0.1) + uF3f4c * PI * a) * 32768.0)));
    if (uSw1 == 2) return s1 / (1.0 + abs(sl((2.0 * PI * uF3f48 / (s1 + 0.1) + uF3f4c * PI * a) * 32768.0)));
    if (uSw1 == 3) return s1 / (1.0 + abs(sl(uF3f50 * s1 * 32768.0) * cl(uF3f54 * a * 32768.0)));
    if (uSw1 == 4) return s1 / (1.0 + abs(cl(uF3f48 * s1 * a * 65536.0)));
    if (uSw1 == 5) return s1 / (1.0 + abs(sl(log(1.0 + s1) * uF3f48 * 32768.0)));
    if (uSw1 == 6) return s1 * ((1.0 - uF3f58) + abs(sl(uF3f5c * a * 32768.0)) * uF3f58);
    if (uSw1 == 7) return s1 / (2.0 + cl(65536.0 * u * v));
    if (uSw1 == 8) return s1 / (2.0 + sl(65536.0 * u * v));
    return s1;
}

float secondSwitch(float s1, float a, float u, float v) {
    float r = s1;
    if (uSw2 == 0) r = safePow(abs(u) + 0.0001, abs(sl(uF3efc * a) * uF3f00 + uF3f08 + sl((uF3f04 + v) * 32768.0)));
    else if (uSw2 == 1) r = 1.0 / (2.0 + sl(uF3ee4 * s1 * 32768.0) * (1.0 - s1) * sl(uF3ee8 * a * 32768.0));
    else if (uSw2 == 2) r = abs(cl(uF3ed8 * a * 32768.0)) / (1.0 + abs(uF3edc * s1));
    else if (uSw2 == 3) r = 1.0 / (2.0 + sl(uF3ee4 * s1 * 32768.0) * s1 * sl(uF3ee8 * a * 32768.0));
    else if (uSw2 == 4) r = 1.0 / (1.0 + abs(sl(log(0.02 + abs(u * v) + s1) * uF3ee0 * 32768.0)));
    else if (uSw2 == 5) r = (1.0 - uF3f58) + uF3f58 * abs(sl(uF3efc * a + uF3ef8 * 32768.0));
    else if (uSw2 == 6) r = 1.0 / (2.0 + cl(65536.0 * u * v * uF3ef4));
    else if (uSw2 == 7) r = 1.0 / (2.0 + sl(65536.0 * u * v * uF3ef8));
    else if (uSw2 == 8) r = abs((1.2 + 0.2 * s1) / (1.0 + abs(u * v)));
    else if (uSw2 == 9) r = abs(cos((s1 + 0.05) / max(abs(uF3f40), 0.001) + a));
    return sat(r);
}

float thirdSwitch(float s1, float s2, float auxA, float a, float u, float v) {
    float r = s2;
    if (uSw3 == 0) r = safePow(abs(u) + 0.0001, abs(sl(uF3f30 * a) * uF3f34 + uF3f3c + sl((uF3f38 + v) * 32768.0)));
    else if (uSw3 == 1) r = 1.0 / (2.0 + sl(uF3f18 * s1 * 32768.0) * auxA * sl(uF3f1c * a * 32768.0));
    else if (uSw3 == 2) r = abs(cl(uF3f0c * a * 32768.0)) / (1.0 + abs(uF3f10 * auxA));
    else if (uSw3 == 3) r = 1.0 / (2.0 + sl(uF3f18 * s1 * 32768.0) * auxA * sl(uF3f1c * a * 32768.0) * max(s2, 0.1));
    else if (uSw3 == 4) r = 1.0 / (1.0 + abs(sl(log(0.02 + abs(auxA * u * v) + s1) * uF3f14 * 32768.0)));
    else if (uSw3 == 5) r = (1.0 - uF3f58) + uF3f58 * abs(sl(uF3f28 * a * 32768.0 + uF3f2c * 8192.0));
    else if (uSw3 == 6) r = 1.0 / (1.2 + 0.2 * abs(cl(uF3f20 * u * v * 65536.0)));
    else if (uSw3 == 7) r = 1.0 / (5.0 + 0.5 * abs(sl(uF3f24 * u * v * 65536.0)));
    else if (uSw3 == 8) r = abs(cos((s1 + 0.05) / max(abs(uF3f44), 0.001) + a));
    else if (uSw3 == 9) r = s2;
    return sat(r);
}

bool styleGate(float s1, float a) {
    if (uRhombuses == 0 && uMesh == 0) return true;
    float aa = sl(100.0 * a * 32768.0);
    if (uRhombuses != 0) {
        float bb = sl(50.0 * s1 * s1 * 32768.0);
        return bb > aa;
    }
    float bb = sl(50.0 * s1 * 32768.0);
    return abs(aa - bb) >= 0.25;
}

float lateCombine(float s2, float s3) {
    float e = uF3ecc;
    if (uLateSel == 0) return PROFILE_SCALE * s3;
    if (uLateSel == 1) return PROFILE_SCALE * (e * s2 + (1.0 - e) * s3);
    if (uLateSel == 2) return PROFILE_SCALE / (1.0 + 30.0 * (e * s2 + (1.0 - e) * s3));
    if (uLateSel == 3) return PROFILE_SCALE / (1.0 + 10.0 * e * s2 * s3);
    if (uLateSel == 4) return (PROFILE_SCALE * s2) / (1.0 + 10.0 * e * s3);
    if (uLateSel == 5) return PROFILE_SCALE * safePow(s2, (1.0 + e) * s3);
    return PROFILE_SCALE * s3;
}

float sampleScalar(vec2 p) {
    float hw = 0.5 * uResolution.x;
    float hh = 0.5 * uResolution.y;
    float u = (p.x - hw) / max(hw, 1.0);
    float v = (p.y - hh) / max(hh, 1.0);
    if (uFlipU != 0) u = -u;
    if (uFlipV != 0) v = -v;
    if (uPhasePerturb != 0) v = cos(PI * v * uF3f64 + uF3f70);
    float rawV = v;
    if (uBlendUv != 0) u = 0.5 * (u + v);

    float angle = abs(atan(u, v)) / PI;
    float radius = sat(INV_SQRT2 * (0.45 + 0.9999 * sqrt(u * u + v * v)));
    if (uF3f60 != 0.0 && uSw1 != 8) {
        float stripe = 2.0 * abs(fract(uF3f60 * angle) - 0.5);
        angle = stripe;
        if (uStripeRadiusBlend != 0 && uF3f60 < 5.0 && uSw2 != 9 && uSw3 != 9 && uBlendUv == 0) radius = 0.5 * (radius + stripe);
    }

    if (uCoordMode == 1) {
        float expv = max(round(sl((radius + uF3ed0) * 32768.0) + uF3ed0), 1.0);
        radius = sat(safePow(abs(radius) + 0.45, expv));
    } else if (uCoordMode == 2) {
        vec2 q = applyQuadratic(u, rawV);
        u = q.x; v = q.y;
        radius = sat(0.45 + 0.55 * abs(q.x));
        angle = sat(0.5 + 0.5 * q.y);
    } else if (uCoordMode == 3) {
        radius = sat(0.5 + 0.5 * sl((u + v + uF3ed4) * 32768.0));
    }

    float s1 = firstSwitch(radius, angle, u, v);
    if (uInvertFirst != 0) s1 = 1.0 - s1;
    s1 = sat(s1);
    float s2 = secondSwitch(s1, angle, u, v);
    float auxA = (uLateSel == 0) ? ((1.0 - uF3ecc) + uF3ecc * s2) : 1.0;
    float s3 = thirdSwitch(s1, s2, auxA, angle, u, v);
    float coord = radius * PROFILE_SCALE;
    if (styleGate(s1, angle)) coord = lateCombine(s2, s3);
    return sampleProfile(clamp(coord, 0.0, PROFILE_SCALE)) * max(s1, 0.001);
}

void main() {
    vec2 p = vec2(gl_FragCoord.x, uResolution.y - gl_FragCoord.y);
    outScalar = sampleScalar(p);
}
"#;

const COLOR_FS: &str = r#"#version 330 core
out vec4 fragColor;
uniform sampler2D uScalar;
uniform sampler2D uPalette;
uniform vec2 uResolution;
uniform float uPaletteScroll;
uniform float uPalettePhase;
uniform float uNormMax;

float scalarFetch(ivec2 p) {
    p = clamp(p, ivec2(0), ivec2(uResolution) - ivec2(1));
    return texelFetch(uScalar, p, 0).r;
}

vec3 colorForScalar(float value) {
    float n = pow(clamp(value / max(uNormMax, 1.0), 0.0, 1.0), 0.82);
    float idx = fract((n * 1023.0 + uPaletteScroll - uPalettePhase) / 1024.0);
    return texture(uPalette, vec2(idx, 0.5)).rgb;
}

float scalarLinear(vec2 p) {
    vec2 uv = (p + vec2(0.5)) / uResolution;
    return texture(uScalar, uv).r;
}

void main() {
    ivec2 ip = ivec2(gl_FragCoord.xy);
    float c = scalarFetch(ip);
    float dx = abs(scalarFetch(ip + ivec2(1, 0)) - scalarFetch(ip - ivec2(1, 0)));
    float dy = abs(scalarFetch(ip + ivec2(0, 1)) - scalarFetch(ip - ivec2(0, 1)));
    float da = abs(scalarFetch(ip + ivec2(1, 1)) - scalarFetch(ip - ivec2(1, 1)));
    float db = abs(scalarFetch(ip + ivec2(-1, 1)) - scalarFetch(ip + ivec2(1, -1)));
    float edge = max(max(dx, dy), max(da, db));

    vec3 color;
    if (edge > uNormMax * 0.018) {
        vec2 p = gl_FragCoord.xy;
        color = (
            colorForScalar(scalarLinear(p + vec2(-0.25, -0.25))) +
            colorForScalar(scalarLinear(p + vec2( 0.25, -0.25))) +
            colorForScalar(scalarLinear(p + vec2(-0.25,  0.25))) +
            colorForScalar(scalarLinear(p + vec2( 0.25,  0.25)))
        ) * 0.25;
    } else {
        color = colorForScalar(c);
    }
    fragColor = vec4(color, 1.0);
}
"#;

const PALETTES_JSON: &str = include_str!("../palettes.json");

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_all_original_palettes() {
        let palettes = parse_palettes_json(PALETTES_JSON).unwrap();
        assert_eq!(palettes.len(), 46);
        assert_eq!(palettes.first().unwrap().name, "Camomile");
        assert_eq!(palettes.last().unwrap().name, "Ice III");
        assert!(palettes.iter().all(|pal| !pal.nodes.is_empty()));
    }
}
