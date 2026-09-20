//! winit + Blade event loop. Idle Wait; workers wake via user events.

use std::sync::Arc;

use anyhow::Context;
use blade_egui as be;
use blade_graphics as bg;
use winit::application::ApplicationHandler;
use winit::event::WindowEvent;
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::window::{Window, WindowId};

use crate::ui::Desktop;

enum UserEvent {
    #[allow(dead_code)]
    Wake,
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

struct App {
    ctx: egui::Context,
    desktop: Desktop,
    runtime: Option<Runtime>,
    error: Option<anyhow::Error>,
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
                let raw = {
                    let runtime = self.runtime.as_mut().unwrap();
                    runtime.input.take_egui_input(&runtime.window)
                };
                let output = {
                    let desktop = &mut self.desktop;
                    self.ctx.run_ui(raw, |ui| {
                        desktop.show(ui);
                    })
                };
                if let Err(error) = self.runtime.as_mut().unwrap().paint(&self.ctx, output) {
                    self.error = Some(error);
                    event_loop.exit();
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        event_loop.set_control_flow(ControlFlow::Wait);
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, _event: UserEvent) {
        if let Some(runtime) = &self.runtime {
            runtime.window.request_redraw();
        }
    }
}

pub fn run_demo() -> anyhow::Result<()> {
    let (account, page) = crate::session::demo_state()?;
    let desktop = Desktop::from_demo(account, page);
    let event_loop = EventLoop::<UserEvent>::with_user_event().build()?;
    let ctx = egui::Context::default();
    let _wake = Arc::new(|| {});
    let mut app = App {
        ctx,
        desktop,
        runtime: None,
        error: None,
    };
    event_loop.run_app(&mut app)?;
    if let Some(error) = app.error {
        return Err(error);
    }
    Ok(())
}
