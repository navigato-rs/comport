//! winit + Blade event loop. Idle Wait; workers wake via user events.

use std::sync::Arc;

use anyhow::Context;
use blade_egui as be;
use blade_graphics as bg;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

use crate::cache::Cache;
use crate::https::Https;
use crate::session::{Event, Session};
use crate::ui::{Desktop, LoginForm};

enum UserEvent {
    Wake,
}

pub struct Startup {
    pub demo: bool,
    pub site: Option<String>,
}

struct Runtime {
    window: Window,
    context: bg::Context,
    surface: bg::Surface,
    surface_info: bg::SurfaceInfo,
    encoder: bg::CommandEncoder,
    last_sync: Option<bg::SyncPoint>,
    pending_view: Option<bg::TextureView>,
    painter: be::GuiPainter,
    input: egui_winit::State,
    size: winit::dpi::PhysicalSize<u32>,
}

impl Runtime {
    fn new(event_loop: &ActiveEventLoop, ctx: &egui::Context) -> anyhow::Result<Self> {
        #[allow(unused_mut)]
        let mut attributes = Window::default_attributes()
            .with_title("ComPort")
            .with_inner_size(winit::dpi::LogicalSize::new(1280.0, 800.0))
            .with_min_inner_size(winit::dpi::LogicalSize::new(960.0, 640.0));
        #[cfg(target_os = "linux")]
        {
            use winit::platform::{
                wayland::WindowAttributesExtWayland, x11::WindowAttributesExtX11,
            };
            attributes = WindowAttributesExtWayland::with_name(attributes, "comport", "comport");
            attributes = WindowAttributesExtX11::with_name(attributes, "comport", "comport");
        }
        let window = event_loop
            .create_window(attributes)
            .context("create window")?;
        let context = unsafe {
            bg::Context::init(bg::ContextDesc {
                presentation: true,
                validation: cfg!(debug_assertions),
                ..Default::default()
            })
        }
        .map_err(|error| anyhow::anyhow!("GPU initialization failed: {error:?}"))?;
        let size = window.inner_size();
        let config = bg::SurfaceConfig {
            size: bg::Extent {
                width: size.width.max(1),
                height: size.height.max(1),
                depth: 1,
            },
            usage: bg::TextureUsage::TARGET,
            ..Default::default()
        };
        let surface = context
            .create_surface_configured(&window, config)
            .map_err(|error| anyhow::anyhow!("GPU surface creation failed: {error:?}"))?;
        let surface_info = surface.info();
        let painter = be::GuiPainter::new(surface_info, &context);
        let encoder = context.create_command_encoder(bg::CommandEncoderDesc {
            name: "comport",
            buffer_count: 1,
        });
        let input = egui_winit::State::new(
            ctx.clone(),
            egui::ViewportId::ROOT,
            &window,
            Some(window.scale_factor() as f32),
            None,
            None,
        );
        Ok(Self {
            window,
            context,
            surface,
            surface_info,
            encoder,
            last_sync: None,
            pending_view: None,
            painter,
            input,
            size,
        })
    }

    fn paint(&mut self, ctx: &egui::Context, output: egui::FullOutput) -> anyhow::Result<()> {
        self.input
            .handle_platform_output(&self.window, output.platform_output);
        let jobs = ctx.tessellate(output.shapes, output.pixels_per_point);
        let screen = be::ScreenDescriptor {
            physical_size: (self.size.width, self.size.height),
            scale_factor: output.pixels_per_point,
        };
        if let Some(ref sync) = self.last_sync {
            self.context
                .wait_for(sync, !0)
                .map_err(|error| anyhow::anyhow!("GPU frame wait failed: {error:?}"))?;
        }
        if let Some(view) = self.pending_view.take() {
            self.context.destroy_texture_view(view);
        }
        self.encoder.start();
        self.painter
            .update_textures(&mut self.encoder, &output.textures_delta, &self.context);
        let frame = self.surface.acquire_frame();
        self.encoder.init_texture(frame.texture());
        let view = self.context.create_texture_view(
            frame.texture(),
            bg::TextureViewDesc {
                name: "comport surface",
                format: self.surface_info.format,
                dimension: bg::ViewDimension::D2,
                subresources: &bg::TextureSubresources::default(),
            },
        );
        {
            let mut pass = self.encoder.render(
                "comport",
                bg::RenderTargetSet {
                    colors: &[bg::RenderTarget {
                        view,
                        init_op: bg::InitOp::Clear(bg::TextureColor::OpaqueBlack),
                        finish_op: bg::FinishOp::Store,
                    }],
                    depth_stencil: None,
                },
            );
            self.painter.paint(&mut pass, &jobs, &screen, &self.context);
        }
        self.encoder.present(frame);
        self.last_sync = Some(self.context.submit(&mut self.encoder));
        self.painter.after_submit(self.last_sync.as_ref().unwrap());
        self.pending_view = Some(view);
        Ok(())
    }
}

enum Screen {
    Login(LoginForm),
    Chat(Box<Desktop>),
}

struct App {
    ctx: egui::Context,
    screen: Screen,
    session: Option<Session>,
    runtime: Option<Runtime>,
    error: Option<anyhow::Error>,
    proxy: winit::event_loop::EventLoopProxy<UserEvent>,
}

impl App {
    fn submit_login(&mut self, form: &LoginForm) -> anyhow::Result<()> {
        let https = Https::from_system_roots()?;
        if let Some(parent) = crate::settings::cache_path().parent() {
            std::fs::create_dir_all(parent)?;
        }
        let cache = Cache::open(
            crate::settings::cache_path()
                .to_str()
                .unwrap_or("cache.sqlite"),
        )?;
        let proxy = self.proxy.clone();
        let wake = Arc::new(move || {
            let _ = proxy.send_event(UserEvent::Wake);
        });
        let session = Session::spawn_waking(Arc::new(https), cache, wake);
        session.login_full(
            &form.site,
            &form.login_id,
            &form.password,
            &form.totp,
            &form.pat,
        )?;
        self.session = Some(session);
        Ok(())
    }

    fn drain_session(&mut self) {
        let Some(session) = self.session.as_ref() else {
            return;
        };
        loop {
            match session.try_recv() {
                Ok(Some(Event::Ready { account })) => {
                    let _ = crate::settings::save(&crate::settings::Settings {
                        site_url: account.site_url.clone(),
                    });
                    if let Some(room) = account
                        .sidebar
                        .grouped()
                        .into_iter()
                        .flat_map(|(_, rooms)| rooms)
                        .next()
                    {
                        let id = room.id.clone();
                        let _ = session.load_history(&id);
                    }
                    let empty = crate::core::MessagePage {
                        channel_id: String::new(),
                        messages: Vec::new(),
                        from_cache: true,
                        loaded_on: std::thread::current().id(),
                    };
                    self.screen = Screen::Chat(Box::new(Desktop::from_demo(account, empty)));
                }
                Ok(Some(Event::Messages { page })) => {
                    if let Screen::Chat(desktop) = &mut self.screen {
                        desktop.selected = page.channel_id.clone();
                        desktop.page = page;
                    }
                    if let Screen::Login(form) = &mut self.screen {
                        form.busy = false;
                    }
                }
                Ok(Some(Event::Sidebar { sidebar })) => {
                    if let Screen::Chat(desktop) = &mut self.screen {
                        desktop.account.sidebar = sidebar;
                    }
                }
                Ok(Some(Event::Error { message })) => {
                    if message.contains("mfa_required") {
                        if let Screen::Login(form) = &mut self.screen {
                            form.need_mfa = true;
                            form.busy = false;
                            form.error = Some("Enter the MFA code from your authenticator.".into());
                        }
                    } else if let Screen::Login(form) = &mut self.screen {
                        form.busy = false;
                        form.error = Some(message);
                    }
                    self.session = None;
                    break;
                }
                Ok(None) => break,
                Err(_) => {
                    self.session = None;
                    break;
                }
            }
        }
    }
}

impl ApplicationHandler<UserEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.runtime.is_some() {
            return;
        }
        match Runtime::new(event_loop, &self.ctx) {
            Ok(runtime) => {
                runtime.window.request_redraw();
                self.runtime = Some(runtime);
            }
            Err(error) => {
                self.error = Some(error);
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        if let Some(runtime) = self.runtime.as_mut() {
            let response = runtime.input.on_window_event(&runtime.window, &event);
            if response.repaint {
                runtime.window.request_redraw();
            }
        } else {
            return;
        }
        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::RedrawRequested => {
                self.drain_session();
                let raw = {
                    let runtime = self.runtime.as_mut().unwrap();
                    runtime.input.take_egui_input(&runtime.window)
                };
                let mut login_submit = false;
                let mut room_click = None;
                let output = {
                    self.ctx.run_ui(raw, |ui| match &mut self.screen {
                        Screen::Login(form) => {
                            login_submit = form.show(ui);
                        }
                        Screen::Chat(desktop) => {
                            room_click = desktop.show(ui);
                        }
                    })
                };
                if login_submit && let Screen::Login(form) = &mut self.screen {
                    form.busy = true;
                    form.error = None;
                    let snapshot = LoginForm {
                        site: form.site.clone(),
                        login_id: form.login_id.clone(),
                        password: form.password.clone(),
                        totp: form.totp.clone(),
                        pat: form.pat.clone(),
                        error: None,
                        busy: true,
                        need_mfa: form.need_mfa,
                    };
                    if let Err(error) = self.submit_login(&snapshot)
                        && let Screen::Login(form) = &mut self.screen
                    {
                        form.busy = false;
                        form.error = Some(format!("{error:#}"));
                    }
                }
                if let Some(id) = room_click
                    && let Some(session) = &self.session
                {
                    let _ = session.load_history(&id);
                }
                if let Err(error) = self.runtime.as_mut().unwrap().paint(&self.ctx, output) {
                    self.error = Some(error);
                    event_loop.exit();
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        self.drain_session();
        event_loop.set_control_flow(ControlFlow::Wait);
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, _event: UserEvent) {
        if let Some(runtime) = &self.runtime {
            runtime.window.request_redraw();
        }
    }
}

pub fn run(startup: Startup) -> anyhow::Result<()> {
    if startup.demo {
        return run_demo();
    }
    let saved = crate::settings::load();
    let site = startup.site.unwrap_or(saved.site_url);
    let event_loop = EventLoop::<UserEvent>::with_user_event().build()?;
    let proxy = event_loop.create_proxy();
    let ctx = egui::Context::default();
    let mut app = App {
        ctx,
        screen: Screen::Login(LoginForm::new(site)),
        session: None,
        runtime: None,
        error: None,
        proxy,
    };
    event_loop.run_app(&mut app)?;
    if let Some(error) = app.error {
        return Err(error);
    }
    Ok(())
}

pub fn run_demo() -> anyhow::Result<()> {
    let (account, page) = crate::session::demo_state()?;
    let event_loop = EventLoop::<UserEvent>::with_user_event().build()?;
    let proxy = event_loop.create_proxy();
    let ctx = egui::Context::default();
    let mut app = App {
        ctx,
        screen: Screen::Chat(Box::new(Desktop::from_demo(account, page))),
        session: None,
        runtime: None,
        error: None,
        proxy,
    };
    event_loop.run_app(&mut app)?;
    if let Some(error) = app.error {
        return Err(error);
    }
    Ok(())
}
