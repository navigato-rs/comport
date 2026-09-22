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
use crate::core::Link;
use crate::handoff::LoginSocket;
use crate::https::Https;
use crate::session::{Event, Session};
use crate::ui::{ChatAction, Desktop, LoginAction, LoginForm};

enum UserEvent {
    Wake,
    Handoff(String),
}

pub struct Startup {
    pub demo: bool,
    pub site: Option<String>,
    pub exit_after_frames: Option<u32>,
    pub handoff: Option<String>,
}

struct Runtime {
    surface: bg::Surface,
    surface_info: bg::SurfaceInfo,
    encoder: bg::CommandEncoder,
    last_sync: Option<bg::SyncPoint>,
    pending_view: Option<bg::TextureView>,
    painter: be::GuiPainter,
    input: egui_winit::State,
    size: winit::dpi::PhysicalSize<u32>,
    // Native window and GPU context must outlive surface teardown.
    window: Window,
    context: bg::Context,
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
            surface,
            surface_info,
            encoder,
            last_sync: None,
            pending_view: None,
            painter,
            input,
            size,
            window,
            context,
        })
    }

    fn finish_frame(&mut self) -> anyhow::Result<()> {
        if let Some(ref sync) = self.last_sync {
            self.context
                .wait_for(sync, !0)
                .map_err(|error| anyhow::anyhow!("GPU frame wait failed: {error:?}"))?;
        }
        if let Some(view) = self.pending_view.take() {
            self.context.destroy_texture_view(view);
        }
        self.last_sync = None;
        Ok(())
    }

    fn paint(&mut self, ctx: &egui::Context, output: egui::FullOutput) -> anyhow::Result<()> {
        self.input
            .handle_platform_output(&self.window, output.platform_output);
        let jobs = ctx.tessellate(output.shapes, output.pixels_per_point);
        let screen = be::ScreenDescriptor {
            physical_size: (self.size.width, self.size.height),
            scale_factor: output.pixels_per_point,
        };
        self.finish_frame()?;
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

impl Drop for Runtime {
    fn drop(&mut self) {
        if let Err(error) = self.finish_frame() {
            log::error!("{error}");
        }
        // Swapchain teardown waits for presentation. Keep the native window
        // and event loop alive until this returns (Starcom Runtime::drop).
        self.context.destroy_surface(&mut self.surface);
        self.context.destroy_command_encoder(&mut self.encoder);
        self.painter.destroy(&self.context);
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
    exit_after_frames: Option<u32>,
    frames_painted: u32,
    pending_handoff: Option<String>,
    _login_socket: Option<LoginSocket>,
}

impl App {
    fn ensure_session(&mut self) -> anyhow::Result<()> {
        if self.session.is_some() {
            return Ok(());
        }
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
        self.session = Some(Session::spawn_waking(Arc::new(https), cache, wake));
        Ok(())
    }

    fn site_hint(&self) -> String {
        match &self.screen {
            Screen::Login(form) => form.site.clone(),
            Screen::Chat(desktop) => desktop.account.site_url.clone(),
        }
    }

    fn take_handoff(&mut self) {
        let Some(raw) = self.pending_handoff.take() else {
            return;
        };
        let site = self.site_hint();
        if let Err(error) = self.ensure_session() {
            if let Screen::Login(form) = &mut self.screen {
                form.busy = false;
                form.error = Some(format!("{error:#}"));
            }
            return;
        }
        if let Some(session) = &self.session {
            let _ = session.complete_handoff(&raw, &site);
        }
        if let Screen::Login(form) = &mut self.screen {
            form.busy = true;
            form.error = None;
        }
    }

    fn apply_login(&mut self, action: LoginAction) {
        let request = match action {
            LoginAction::None => return,
            LoginAction::Browser => {
                let site = self.site_hint();
                Some(LoginRequest::Browser(site))
            }
            LoginAction::Password => {
                let Screen::Login(form) = &self.screen else {
                    return;
                };
                Some(LoginRequest::Password {
                    site: form.site.clone(),
                    login_id: form.login_id.clone(),
                    password: form.password.clone(),
                    totp: form.totp.clone(),
                    pat: form.pat.clone(),
                })
            }
            LoginAction::Paste => {
                let Screen::Login(form) = &self.screen else {
                    return;
                };
                Some(LoginRequest::Paste {
                    raw: form.paste.clone(),
                    site: form.site.clone(),
                })
            }
        };
        let Some(request) = request else {
            return;
        };
        if let Err(error) = self.ensure_session() {
            if let Screen::Login(form) = &mut self.screen {
                form.busy = false;
                form.error = Some(format!("{error:#}"));
            }
            return;
        }
        if let Screen::Login(form) = &mut self.screen {
            form.busy = true;
            form.error = None;
        }
        let result = self.session.as_ref().map(|session| match request {
            LoginRequest::Browser(site) => session.browser_login(&site),
            LoginRequest::Password {
                site,
                login_id,
                password,
                totp,
                pat,
            } => session.login_full(&site, &login_id, &password, &totp, &pat),
            LoginRequest::Paste { raw, site } => session.complete_handoff(&raw, &site),
        });
        if let Some(Err(error)) = result
            && let Screen::Login(form) = &mut self.screen
        {
            form.busy = false;
            form.error = Some(format!("{error:#}"));
        }
    }

    fn apply_chat(&mut self, action: ChatAction) {
        if self.session.is_none() {
            return;
        }
        if matches!(action, ChatAction::Open(_))
            && let Screen::Chat(desktop) = &mut self.screen
        {
            desktop.editing = None;
        }
        let Some(session) = self.session.as_ref() else {
            return;
        };
        match action {
            ChatAction::None => {}
            ChatAction::Open(id) => {
                let _ = session.load_history(&id);
            }
            ChatAction::Send {
                channel_id,
                text,
                editing,
            } => {
                let _ = session.send_message(&channel_id, &text, editing);
            }
            ChatAction::Delete { post_id } => {
                let _ = session.delete_message(&post_id);
            }
            ChatAction::SwitchTeam => {
                let _ = session.switch_team();
            }
        }
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
                    let mut desktop = Desktop::from_demo(account, empty);
                    desktop.link = Link::Connecting;
                    self.screen = Screen::Chat(Box::new(desktop));
                }
                Ok(Some(Event::Account { account })) => {
                    if let Screen::Chat(desktop) = &mut self.screen {
                        desktop.account = account;
                        desktop.page.messages.clear();
                        desktop.page.channel_id.clear();
                        desktop.selected.clear();
                        desktop.editing = None;
                        desktop.draft.clear();
                    }
                    if let Some(session) = self.session.as_ref()
                        && let Screen::Chat(desktop) = &self.screen
                        && let Some(room) = desktop
                            .account
                            .sidebar
                            .grouped()
                            .into_iter()
                            .flat_map(|(_, rooms)| rooms)
                            .next()
                    {
                        let id = room.id.clone();
                        let _ = session.load_history(&id);
                    }
                }
                Ok(Some(Event::Messages { page })) => {
                    if let Screen::Chat(desktop) = &mut self.screen
                        && (desktop.selected.is_empty() || desktop.selected == page.channel_id)
                    {
                        desktop.selected = page.channel_id.clone();
                        desktop.page = page;
                    }
                    if let Screen::Login(form) = &mut self.screen {
                        form.busy = false;
                    }
                }
                Ok(Some(Event::Upsert { message })) => {
                    if let Screen::Chat(desktop) = &mut self.screen
                        && message.channel_id == desktop.selected
                    {
                        if let Some(existing) = desktop
                            .page
                            .messages
                            .iter_mut()
                            .find(|item| item.id == message.id)
                        {
                            *existing = message;
                        } else {
                            desktop.page.messages.push(message);
                            desktop.page.messages.sort_by_key(|item| item.create_at);
                        }
                        desktop.updated = true;
                    }
                }
                Ok(Some(Event::Removed {
                    channel_id,
                    post_id,
                })) => {
                    if let Screen::Chat(desktop) = &mut self.screen
                        && desktop.selected == channel_id
                    {
                        desktop.page.messages.retain(|item| item.id != post_id);
                        desktop.updated = true;
                    }
                }
                Ok(Some(Event::Link { state, updated })) => {
                    if let Screen::Chat(desktop) = &mut self.screen {
                        desktop.link = state;
                        if state != Link::Live {
                            desktop.updated = false;
                        } else if updated {
                            desktop.updated = true;
                        }
                    }
                }
                Ok(Some(Event::WaitingBrowser { providers })) => {
                    if let Screen::Login(form) = &mut self.screen {
                        form.busy = false;
                        form.waiting_browser = true;
                        form.providers = providers;
                        form.error = None;
                    }
                }
                Ok(Some(Event::AuthExpired)) => {
                    let site = self.site_hint();
                    let mut form = LoginForm::new(site);
                    form.error = Some("Session expired. Sign in with the browser again.".into());
                    self.screen = Screen::Login(form);
                }
                Ok(Some(Event::Sidebar { sidebar })) => {
                    if let Screen::Chat(desktop) = &mut self.screen {
                        desktop.account.sidebar = sidebar;
                    }
                }
                Ok(Some(Event::Error { message, login })) => {
                    if login && message.contains("mfa_required") {
                        if let Screen::Login(form) = &mut self.screen {
                            form.need_mfa = true;
                            form.busy = false;
                            form.waiting_browser = false;
                            form.error = Some("Enter the MFA code from your authenticator.".into());
                        }
                    } else if login {
                        if let Screen::Login(form) = &mut self.screen {
                            form.busy = false;
                            form.error = Some(message);
                        }
                    } else if let Screen::Chat(desktop) = &mut self.screen {
                        desktop.notice = Some(message);
                    }
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

    fn shutdown(&mut self) {
        if let Screen::Chat(desktop) = &mut self.screen {
            desktop.release_gpu_textures();
        }
        // Drop GPU resources while the event loop and native window still exist.
        self.runtime.take();
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
            WindowEvent::CloseRequested => {
                self.shutdown();
                event_loop.exit();
            }
            WindowEvent::RedrawRequested => {
                self.drain_session();
                self.take_handoff();
                let raw = {
                    let runtime = self.runtime.as_mut().unwrap();
                    runtime.input.take_egui_input(&runtime.window)
                };
                let mut login_action = LoginAction::None;
                let mut chat_action = ChatAction::None;
                let output = {
                    self.ctx.run_ui(raw, |ui| match &mut self.screen {
                        Screen::Login(form) => {
                            login_action = form.show(ui);
                        }
                        Screen::Chat(desktop) => {
                            chat_action = desktop.show(ui);
                        }
                    })
                };
                self.apply_login(login_action);
                self.apply_chat(chat_action);
                if let Err(error) = self.runtime.as_mut().unwrap().paint(&self.ctx, output) {
                    self.error = Some(error);
                    self.shutdown();
                    event_loop.exit();
                    return;
                }
                self.frames_painted += 1;
                if self
                    .exit_after_frames
                    .is_some_and(|n| self.frames_painted >= n)
                {
                    self.shutdown();
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

    fn exiting(&mut self, _event_loop: &ActiveEventLoop) {
        self.shutdown();
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: UserEvent) {
        if let UserEvent::Handoff(url) = event {
            self.pending_handoff = Some(url);
        }
        if let Some(runtime) = &self.runtime {
            runtime.window.request_redraw();
        }
    }
}

enum LoginRequest {
    Browser(String),
    Password {
        site: String,
        login_id: String,
        password: String,
        totp: String,
        pat: String,
    },
    Paste {
        raw: String,
        site: String,
    },
}

pub fn run(startup: Startup) -> anyhow::Result<()> {
    if startup.demo {
        return run_demo_inner(startup.exit_after_frames);
    }
    let saved = crate::settings::load();
    let site = startup.site.unwrap_or(saved.site_url);
    let event_loop = EventLoop::<UserEvent>::with_user_event().build()?;
    let proxy = event_loop.create_proxy();
    let listen_proxy = event_loop.create_proxy();
    let login_socket = LoginSocket::spawn(move |url| {
        let _ = listen_proxy.send_event(UserEvent::Handoff(url));
    })
    .ok();
    let ctx = egui::Context::default();
    let mut app = App {
        ctx,
        screen: Screen::Login(LoginForm::new(site)),
        session: None,
        runtime: None,
        error: None,
        proxy,
        exit_after_frames: startup.exit_after_frames,
        frames_painted: 0,
        pending_handoff: startup.handoff,
        _login_socket: login_socket,
    };
    let result = event_loop.run_app(&mut app);
    app.shutdown();
    result?;
    if let Some(error) = app.error {
        return Err(error);
    }
    Ok(())
}

pub fn run_demo() -> anyhow::Result<()> {
    run_demo_inner(None)
}

fn run_demo_inner(exit_after_frames: Option<u32>) -> anyhow::Result<()> {
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
        exit_after_frames,
        frames_painted: 0,
        pending_handoff: None,
        _login_socket: None,
    };
    let result = event_loop.run_app(&mut app);
    app.shutdown();
    result?;
    if let Some(error) = app.error {
        return Err(error);
    }
    Ok(())
}
