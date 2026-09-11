//! 设置窗口：egui（egui_glow + glutin）直接挂在 winit 0.30 主事件循环上。
//! 配置三组热键、键延迟、settle、接收超时、文本上限；保存时做范围/解析校验。

use std::num::NonZeroU32;
use std::sync::Arc;

use egui::{Align, Context, Layout};
use glutin::config::ConfigTemplateBuilder;
use glutin::context::{ContextAttributesBuilder, NotCurrentGlContext, PossiblyCurrentContext};
use glutin::display::{GetGlDisplay, GlDisplay};
use glutin::surface::{GlSurface, Surface, SurfaceAttributesBuilder, SwapInterval, WindowSurface};
use glutin_winit::{DisplayBuilder, GlWindow};
use raw_window_handle::HasWindowHandle;
use winit::event::WindowEvent;
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{KeyCode, ModifiersState, PhysicalKey};
use winit::window::Window;

use egui_glow::glow;
use egui_glow::EguiGlow;

use crate::config::Config;

/// 窗口关闭时回传给主循环：保存（新配置）或关闭（放弃）。
#[derive(Debug)]
pub enum SettingsOutcome {
    Saved(Config),
    Closed,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum CaptureField {
    Send,
    Recv,
    Stop,
}

pub struct SettingsWindow {
    window: Window,
    gl_surface: Surface<WindowSurface>,
    // 必须持有：context 存活期间 GL 才有效。
    _gl_context: PossiblyCurrentContext,
    egui: EguiGlow,
    draft: Config,
    capture: Option<CaptureField>,
    outcome: Option<SettingsOutcome>,
}

impl SettingsWindow {
    pub fn open(elwt: &ActiveEventLoop, cfg: &Config) -> Result<Self, String> {
        let window_attributes = Window::default_attributes()
            .with_title("ClipBeam 设置")
            .with_inner_size(winit::dpi::LogicalSize::new(480.0, 620.0))
            .with_resizable(false)
            .with_window_icon(Some(crate::tray::window_icon()));

        let display_builder = DisplayBuilder::new().with_window_attributes(Some(window_attributes));
        let (window, gl_config) = display_builder
            .build(
                elwt,
                ConfigTemplateBuilder::new().with_alpha_size(8),
                |mut cs| cs.next().expect("至少存在一个 GL 配置"),
            )
            .map_err(|e| format!("创建设置窗口失败: {e}"))?;
        let window = window.ok_or("winit 未返回窗口")?;

        let gl_display = gl_config.display();
        let raw_handle = window.window_handle().map(|h| h.as_raw()).ok();
        let context_attrs = ContextAttributesBuilder::new().build(raw_handle);
        let not_current = unsafe {
            gl_display
                .create_context(&gl_config, &context_attrs)
                .map_err(|e| format!("创建 GL 上下文失败: {e}"))?
        };
        let surface_attrs = window
            .build_surface_attributes(SurfaceAttributesBuilder::<WindowSurface>::new())
            .map_err(|e| format!("创建 GL 表面失败: {e}"))?;
        let gl_surface = unsafe {
            gl_display
                .create_window_surface(&gl_config, &surface_attrs)
                .map_err(|e| format!("创建窗口表面失败: {e}"))?
        };
        let gl_context = not_current
            .make_current(&gl_surface)
            .map_err(|e| format!("激活 GL 上下文失败: {e}"))?;
        gl_surface
            .set_swap_interval(&gl_context, SwapInterval::Wait(NonZeroU32::new(1).unwrap()))
            .ok();

        let glow_ctx = unsafe {
            glow::Context::from_loader_function_cstr(|name| gl_display.get_proc_address(name))
        };
        let egui = EguiGlow::new(elwt, Arc::new(glow_ctx), None, None, false);
        install_cjk_font(&egui);

        window.focus_window();
        Ok(Self {
            window,
            gl_surface,
            _gl_context: gl_context,
            egui,
            draft: cfg.clone(),
            capture: None,
            outcome: None,
        })
    }

    pub fn focus(&self) {
        self.window.focus_window();
    }

    pub fn window_id(&self) -> winit::window::WindowId {
        self.window.id()
    }

    pub fn take_outcome(&mut self) -> Option<SettingsOutcome> {
        self.outcome.take()
    }

    /// 处理一个窗口事件；返回 true 表示窗口请求重绘。
    pub fn handle_event(&mut self, event: &WindowEvent, modifiers: ModifiersState) -> bool {
        // 热键捕获优先截获按键。
        if let WindowEvent::KeyboardInput { event: ke, .. } = event {
            if self.capture.is_some()
                && ke.state == winit::event::ElementState::Pressed
                && !ke.repeat
            {
                if let PhysicalKey::Code(code) = ke.physical_key {
                    if let Some(spec) = hotkey_spec(code, modifiers) {
                        let field = self.capture.take().unwrap();
                        match field {
                            CaptureField::Send => self.draft.send_hotkey = spec,
                            CaptureField::Recv => self.draft.recv_hotkey = spec,
                            CaptureField::Stop => self.draft.stop_hotkey = spec,
                        }
                    }
                }
                return true;
            }
        }

        match event {
            WindowEvent::CloseRequested => {
                self.outcome = Some(SettingsOutcome::Closed);
                false
            }
            WindowEvent::Resized(size) => {
                if size.width > 0 && size.height > 0 {
                    self.gl_surface.resize(
                        &self._gl_context,
                        NonZeroU32::new(size.width).unwrap(),
                        NonZeroU32::new(size.height).unwrap(),
                    );
                }
                true
            }
            WindowEvent::RedrawRequested => {
                self.render();
                false
            }
            _ => self.egui.on_window_event(&self.window, event).repaint,
        }
    }

    pub fn request_redraw(&self) {
        self.window.request_redraw();
    }

    fn render(&mut self) {
        let draft = &mut self.draft;
        let capture = &mut self.capture;
        let mut close = false;
        let mut saved: Option<Config> = None;

        self.egui.run(&self.window, |ctx: &Context| {
            egui::CentralPanel::default().show(ctx, |ui| {
                ui.heading("ClipBeam 设置");
                ui.add_space(6.0);
                ui.label(egui::RichText::new("热键（全局，任意焦点下生效）").strong());
                ui.add_space(4.0);
                hotkey_row(
                    ui,
                    "发送（宿主机→远程）",
                    &mut draft.send_hotkey,
                    capture,
                    CaptureField::Send,
                );
                hotkey_row(
                    ui,
                    "接收（远程→宿主机）",
                    &mut draft.recv_hotkey,
                    capture,
                    CaptureField::Recv,
                );
                hotkey_row(
                    ui,
                    "中止（任务运行时生效）",
                    &mut draft.stop_hotkey,
                    capture,
                    CaptureField::Stop,
                );

                ui.add_space(10.0);
                ui.label(egui::RichText::new("时序与阈值").strong());
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label("逐键间隔");
                    ui.add(
                        egui::DragValue::new(&mut draft.key_delay_ms)
                            .range(0..=100)
                            .suffix(" ms"),
                    );
                });
                ui.horizontal(|ui| {
                    ui.label("发送前 settle 等待");
                    ui.add(
                        egui::DragValue::new(&mut draft.settle_ms)
                            .range(0..=5000)
                            .suffix(" ms"),
                    );
                });
                ui.horizontal(|ui| {
                    ui.label("二维码接收超时");
                    ui.add(
                        egui::DragValue::new(&mut draft.receive_timeout_s)
                            .range(5..=3600)
                            .suffix(" 秒"),
                    );
                });
                ui.horizontal(|ui| {
                    ui.label("文本大小上限");
                    ui.add(
                        egui::DragValue::new(&mut draft.max_text_kb)
                            .range(1..=10_240)
                            .suffix(" KB"),
                    );
                });

                // 校验信息
                let mut errs = draft.validate();
                errs.extend(hotkey_parse_errors(draft));
                ui.add_space(8.0);
                if let Some(first) = errs.first() {
                    ui.label(
                        egui::RichText::new(format!("✗ {first}"))
                            .color(egui::Color32::from_rgb(0xff, 0x6b, 0x6b)),
                    );
                }

                // 按钮区：吃掉剩余高度，按钮固定在右下角。
                let avail = ui.available_size();
                ui.allocate_ui_with_layout(avail, Layout::right_to_left(Align::BOTTOM), |ui| {
                    ui.add_space(6.0);
                    let save = ui
                        .add_enabled(errs.is_empty(), egui::Button::new("保存"))
                        .on_hover_text("保存配置并重注册热键");
                    if save.clicked() {
                        saved = Some(draft.clone());
                    }
                    ui.add_space(8.0);
                    if ui.button("取消").clicked() {
                        close = true;
                    }
                });
            });
        });

        if let Some(cfg) = saved {
            self.outcome = Some(SettingsOutcome::Saved(cfg));
            close = true;
        }
        if close {
            self.outcome.get_or_insert(SettingsOutcome::Closed);
        }

        self.egui.paint(&self.window);
        let _ = self.gl_surface.swap_buffers(&self._gl_context);
    }
}

impl Drop for SettingsWindow {
    fn drop(&mut self) {
        self.egui.destroy();
    }
}

fn hotkey_row(
    ui: &mut egui::Ui,
    label: &str,
    spec: &mut String,
    capture: &mut Option<CaptureField>,
    field: CaptureField,
) {
    ui.horizontal(|ui| {
        ui.label(label);
        let capturing = *capture == Some(field);
        let text = if capturing {
            "按下新组合键…（再次点击本行取消）".to_string()
        } else {
            spec.clone()
        };
        let btn = ui.add(
            egui::Button::new(text)
                .selected(capturing)
                .min_size(egui::vec2(190.0, 0.0)),
        );
        if btn.clicked() {
            if capturing {
                *capture = None;
            } else {
                *capture = Some(field);
            }
        }
    });
}

/// 把捕获到的物理键 + 修饰键转成 global-hotkey 可解析的规范字符串。
fn hotkey_spec(code: KeyCode, mods: ModifiersState) -> Option<String> {
    let name = format!("{code:?}");
    // 用 global-hotkey 自身的解析器验证（它接受 "KeyK"/"Escape"/"Digit1" 等 Debug 名称）。
    let mut parts: Vec<&str> = Vec::new();
    #[cfg(target_os = "macos")]
    let primary = "Cmd";
    #[cfg(not(target_os = "macos"))]
    let primary = "Ctrl";
    if mods.super_key() || mods.control_key() {
        parts.push(primary);
    }
    if mods.shift_key() {
        parts.push("Shift");
    }
    if mods.alt_key() {
        parts.push("Alt");
    }
    let spec = if parts.is_empty() {
        name
    } else {
        format!("{}+{name}", parts.join("+"))
    };
    spec.parse::<global_hotkey::hotkey::HotKey>().ok()?;
    Some(spec)
}

/// 除 Config::validate 的范围检查外，再验证三个热键字符串可解析。
fn hotkey_parse_errors(cfg: &Config) -> Vec<String> {
    let mut errs = Vec::new();
    for (name, s) in [
        ("发送热键", &cfg.send_hotkey),
        ("接收热键", &cfg.recv_hotkey),
        ("中止热键", &cfg.stop_hotkey),
    ] {
        if s.trim().is_empty() {
            continue; // 非空错误由 validate 报
        }
        if s.parse::<global_hotkey::hotkey::HotKey>().is_err() {
            errs.push(format!("{name}无法识别：{s}"));
        }
    }
    errs
}

/// egui 内置字体不含中文字形，从系统字体文件补一个 CJK 字体。
fn install_cjk_font(egui: &EguiGlow) {
    let candidates: &[&str] = &[
        "/System/Library/Fonts/PingFang.ttc",
        "/System/Library/Fonts/STHeiti Light.ttc",
        "C:\\Windows\\Fonts\\msyh.ttc",
        "C:\\Windows\\Fonts\\msyh.ttf",
        "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
        "/usr/share/fonts/truetype/noto/NotoSansCJK-Regular.ttc",
    ];
    let Some(path) = candidates.iter().find(|p| std::path::Path::new(p).exists()) else {
        return;
    };
    let Ok(data) = std::fs::read(path) else {
        return;
    };
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "sys_cjk".to_owned(),
        egui::FontData::from_owned(data).into(),
    );
    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        fonts
            .families
            .entry(family)
            .or_default()
            .insert(0, "sys_cjk".to_owned());
    }
    egui.egui_ctx.set_fonts(fonts);
}
