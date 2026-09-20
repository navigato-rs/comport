//! Offscreen Blade snapshot of the same desktop the window would paint.

use std::io;
use std::path::Path;

use anyhow::Context;
use blade_egui as be;
use blade_graphics as bg;

use crate::ui::{Desktop, SNAPSHOT_HEIGHT, SNAPSHOT_WIDTH};

pub fn save_png(desktop: &mut Desktop, path: &Path) -> anyhow::Result<()> {
    // SAFETY: offscreen resources stay on this thread and are destroyed after GPU wait.
    let context = unsafe { bg::Context::init(bg::ContextDesc::default()) }
        .map_err(|error| anyhow::anyhow!("offscreen GPU initialization failed: {error:?}"))?;
    let size = bg::Extent {
        width: SNAPSHOT_WIDTH,
        height: SNAPSHOT_HEIGHT,
        depth: 1,
    };
    let format = bg::TextureFormat::Rgba8UnormSrgb;
    let mut painter = be::GuiPainter::new(
        bg::SurfaceInfo {
            format,
            alpha: bg::AlphaMode::PreMultiplied,
        },
        &context,
    );
    let mut encoder = context.create_command_encoder(bg::CommandEncoderDesc {
        name: "comport snapshot",
        buffer_count: 1,
    });
    let texture = context.create_texture(bg::TextureDesc {
        name: "comport snapshot",
        format,
        size,
        array_layer_count: 1,
        mip_level_count: 1,
        sample_count: 1,
        dimension: bg::TextureDimension::D2,
        usage: bg::TextureUsage::TARGET | bg::TextureUsage::COPY,
        external: None,
    });
    let view = context.create_texture_view(
        texture,
        bg::TextureViewDesc {
            name: "comport snapshot",
            format,
            dimension: bg::ViewDimension::D2,
            subresources: &bg::TextureSubresources::default(),
        },
    );
    let stride = (size.width * 4).div_ceil(256) * 256;
    let buffer = context.create_buffer(bg::BufferDesc {
        name: "comport readback",
        size: u64::from(stride) * u64::from(size.height),
        memory: bg::Memory::Shared,
    });
    let result: anyhow::Result<()> = (|| {
        let ctx = egui::Context::default();
        let mut textures = egui::TexturesDelta::default();
        let mut shapes = Vec::new();
        for pass in 0..3 {
            let screen = egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(size.width as f32, size.height as f32),
            );
            let input = egui::RawInput {
                screen_rect: Some(screen),
                time: Some(pass as f64 / 60.0),
                viewports: [(
                    egui::ViewportId::ROOT,
                    egui::ViewportInfo {
                        native_pixels_per_point: Some(1.0),
                        inner_rect: Some(screen),
                        ..Default::default()
                    },
                )]
                .into_iter()
                .collect(),
                ..Default::default()
            };
            let output = ctx.run_ui(input, |ui| {
                desktop.show(ui);
            });
            textures.append(output.textures_delta);
            shapes = output.shapes;
        }
        let jobs = ctx.tessellate(shapes, 1.0);
        encoder.start();
        encoder.init_texture(texture);
        painter.update_textures(&mut encoder, &textures, &context);
        {
            let mut pass = encoder.render(
                "comport snapshot",
                bg::RenderTargetSet {
                    colors: &[bg::RenderTarget {
                        view,
                        init_op: bg::InitOp::Clear(bg::TextureColor::OpaqueBlack),
                        finish_op: bg::FinishOp::Store,
                    }],
                    depth_stencil: None,
                },
            );
            painter.paint(
                &mut pass,
                &jobs,
                &be::ScreenDescriptor {
                    physical_size: (size.width, size.height),
                    scale_factor: 1.0,
                },
                &context,
            );
        }
        {
            let mut transfer = encoder.transfer("comport readback");
            transfer.copy_texture_to_buffer(
                bg::TexturePiece {
                    texture,
                    mip_level: 0,
                    array_layer: 0,
                    origin: [0, 0, 0],
                },
                buffer.into(),
                stride,
                size,
            );
        }
        let sync = context.submit(&mut encoder);
        painter.after_submit(&sync);
        context
            .wait_for(&sync, !0)
            .map_err(|error| anyhow::anyhow!("snapshot GPU wait failed: {error:?}"))?;
        let mapped = unsafe {
            std::slice::from_raw_parts(buffer.data(), stride as usize * size.height as usize)
        };
        let mut rgba = Vec::with_capacity(size.width as usize * size.height as usize * 4);
        for row in mapped.chunks_exact(stride as usize) {
            rgba.extend_from_slice(&row[..size.width as usize * 4]);
        }
        if let Some(parent) = path
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            std::fs::create_dir_all(parent)?;
        }
        let mut png = png::Encoder::new(
            io::BufWriter::new(std::fs::File::create(path)?),
            size.width,
            size.height,
        );
        png.set_color(png::ColorType::Rgba);
        png.set_depth(png::BitDepth::Eight);
        png.set_compression(png::Compression::High);
        let mut writer = png.write_header()?;
        writer.write_image_data(&rgba)?;
        writer.finish()?;
        Ok(())
    })();
    context.destroy_texture_view(view);
    context.destroy_texture(texture);
    context.destroy_buffer(buffer);
    painter.destroy(&context);
    context.destroy_command_encoder(&mut encoder);
    result.with_context(|| format!("render snapshot to {}", path.display()))
}

pub fn save_demo(path: &Path) -> anyhow::Result<()> {
    let (account, page) = crate::session::demo_state()?;
    let mut desktop = Desktop::from_demo(account, page);
    save_png(&mut desktop, path)
}
