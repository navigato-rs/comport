//! Mattermost-like shell: left room list, favorites, portraits, messages.

use std::collections::HashMap;

use egui::{
    Color32, ColorImage, Context, FontId, RichText, Sense, TextureHandle, TextureOptions, Ui, Vec2,
};

use crate::core::{Message, MessagePage, RoomKind, Sidebar, SidebarSection};
use crate::mattermost::Account;

pub const SNAPSHOT_WIDTH: u32 = 1280;
pub const SNAPSHOT_HEIGHT: u32 = 800;

const SIDEBAR_BG: Color32 = Color32::from_rgb(0x1e, 0x32, 0x5c);
const CENTER_BG: Color32 = Color32::from_rgb(0xff, 0xff, 0xff);
const SIDEBAR_TEXT: Color32 = Color32::from_rgb(0xff, 0xff, 0xff);
const MUTED: Color32 = Color32::from_rgb(0xba, 0xc3, 0xd0);
const ACCENT: Color32 = Color32::from_rgb(0x1c, 0x58, 0xd9);
const GOLD: Color32 = Color32::from_rgb(0xff, 0xbc, 0x1f);

pub struct Desktop {
    pub account: Account,
    pub page: MessagePage,
    pub selected: String,
    textures: HashMap<String, TextureHandle>,
}

impl Desktop {
    pub fn from_demo(account: Account, page: MessagePage) -> Self {
        Self {
            selected: page.channel_id.clone(),
            account,
            page,
            textures: HashMap::new(),
        }
    }

    pub fn show(&mut self, ui: &mut Ui) {
        egui::Panel::left("rooms")
            .exact_size(280.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(
                egui::Frame::NONE
                    .fill(SIDEBAR_BG)
                    .inner_margin(egui::Margin::symmetric(12, 16)),
            )
            .show_inside(ui, |ui| self.sidebar(ui));
        egui::Panel::bottom("compose")
            .resizable(false)
            .exact_size(56.0)
            .show_separator_line(false)
            .frame(egui::Frame::NONE.fill(CENTER_BG))
            .show_inside(ui, |ui| self.compose(ui));
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(CENTER_BG))
            .show_inside(ui, |ui| {
                self.header(ui);
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        self.messages(ui);
                    });
            });
    }

    fn sidebar(&self, ui: &mut Ui) {
        ui.label(
            RichText::new(&self.account.team.display_name)
                .color(SIDEBAR_TEXT)
                .strong()
                .size(16.0),
        );
        ui.add_space(12.0);
        for (section, rooms) in self.account.sidebar.grouped() {
            ui.label(
                RichText::new(section.label())
                    .color(MUTED)
                    .size(11.0)
                    .strong(),
            );
            for room in rooms {
                let selected = room.id == self.selected;
                let mut label = room.display_name.clone();
                if section == SidebarSection::Favorites {
                    label = format!("★ {label}");
                } else if room.kind == RoomKind::Private {
                    label = format!("🔒 {label}");
                }
                let color = if selected { GOLD } else { SIDEBAR_TEXT };
                ui.label(RichText::new(label).color(color).size(14.0));
            }
            ui.add_space(8.0);
        }
    }

    fn header(&self, ui: &mut Ui) {
        let title = self
            .account
            .sidebar
            .room(&self.selected)
            .map(|room| room.display_name.as_str())
            .unwrap_or("Channel");
        egui::Frame::NONE
            .fill(CENTER_BG)
            .inner_margin(egui::Margin::symmetric(16, 12))
            .show(ui, |ui| {
                ui.label(
                    RichText::new(format!("# {title}"))
                        .strong()
                        .size(18.0)
                        .color(Color32::from_rgb(0x1b, 0x1d, 0x22)),
                );
                ui.separator();
            });
    }

    fn messages(&mut self, ui: &mut Ui) {
        let messages = self.page.messages.clone();
        for message in &messages {
            ui.horizontal(|ui| {
                self.portrait(ui, message);
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(
                            RichText::new(&message.author_name)
                                .strong()
                                .color(Color32::from_rgb(0x1b, 0x1d, 0x22)),
                        );
                    });
                    ui.label(
                        RichText::new(&message.body)
                            .size(15.0)
                            .color(Color32::from_rgb(0x3d, 0x3c, 0x40)),
                    );
                });
            });
            ui.add_space(10.0);
        }
    }

    fn portrait(&mut self, ui: &mut Ui, message: &Message) {
        let size = Vec2::splat(36.0);
        if let Some(portrait) = &message.portrait
            && let Some(texture) = self.texture_for(ui.ctx(), portrait)
        {
            ui.image((texture.id(), size));
            return;
        }
        let (rect, _) = ui.allocate_exact_size(size, Sense::hover());
        ui.painter().circle_filled(rect.center(), 18.0, ACCENT);
        let letter = message.author_name.chars().next().unwrap_or('?');
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            letter.to_string(),
            FontId::proportional(16.0),
            Color32::WHITE,
        );
    }

    fn texture_for(
        &mut self,
        ctx: &Context,
        portrait: &crate::core::Portrait,
    ) -> Option<TextureHandle> {
        if let Some(handle) = self.textures.get(&portrait.user_id) {
            return Some(handle.clone());
        }
        let image = decode_png(&portrait.bytes)?;
        let handle = ctx.load_texture(
            format!("avatar-{}", portrait.user_id),
            image,
            TextureOptions::LINEAR,
        );
        self.textures
            .insert(portrait.user_id.clone(), handle.clone());
        Some(handle)
    }

    fn compose(&self, ui: &mut Ui) {
        egui::Frame::NONE
            .inner_margin(egui::Margin::symmetric(16, 12))
            .show(ui, |ui| {
                ui.label(RichText::new("Write to Town Square").color(MUTED));
            });
    }
}

fn decode_png(bytes: &[u8]) -> Option<ColorImage> {
    let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    let mut reader = decoder.read_info().ok()?;
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    let width = info.width as usize;
    let height = info.height as usize;
    let rgba = match info.color_type {
        png::ColorType::Rgb => buf
            .chunks(3)
            .flat_map(|c| [c[0], c[1], c[2], 255])
            .collect(),
        png::ColorType::Rgba => buf,
        _ => return None,
    };
    Some(ColorImage::from_rgba_unmultiplied([width, height], &rgba))
}

/// Layout smoke for tests that cannot open a GPU surface.
pub fn sidebar_has_left_list(sidebar: &Sidebar) -> bool {
    !sidebar.grouped().is_empty()
}
