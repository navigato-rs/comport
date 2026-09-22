//! Mattermost-like shell: left room list, favorites, portraits, messages.

use std::collections::HashMap;

use egui::{
    Color32, ColorImage, Context, FontId, RichText, Sense, TextureHandle, TextureOptions, Ui, Vec2,
};

use crate::core::{Link, Message, MessagePage, RoomKind, Sidebar, SidebarSection};
use crate::mattermost::Account;

pub enum ChatAction {
    None,
    Open(String),
    Send {
        channel_id: String,
        text: String,
        editing: Option<String>,
    },
    Delete {
        post_id: String,
    },
    SwitchTeam,
}

pub enum LoginAction {
    None,
    Browser,
    Password,
    Paste,
}

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
    pub link: Link,
    pub updated: bool,
    pub draft: String,
    pub editing: Option<String>,
    pub notice: Option<String>,
    textures: HashMap<String, TextureHandle>,
}

impl Desktop {
    pub fn from_demo(account: Account, page: MessagePage) -> Self {
        Self {
            selected: page.channel_id.clone(),
            account,
            page,
            link: Link::Live,
            updated: false,
            draft: String::new(),
            editing: None,
            notice: None,
            textures: HashMap::new(),
        }
    }

    pub fn release_gpu_textures(&mut self) {
        self.textures.clear();
    }

    pub fn show(&mut self, ui: &mut Ui) -> ChatAction {
        let mut action = ChatAction::None;
        egui::Panel::left("rooms")
            .exact_size(280.0)
            .resizable(false)
            .show_separator_line(false)
            .frame(
                egui::Frame::NONE
                    .fill(SIDEBAR_BG)
                    .inner_margin(egui::Margin::symmetric(12, 16)),
            )
            .show_inside(ui, |ui| {
                action = self.sidebar(ui);
            });
        egui::Panel::bottom("compose")
            .resizable(false)
            .exact_size(88.0)
            .show_separator_line(false)
            .frame(egui::Frame::NONE.fill(CENTER_BG))
            .show_inside(ui, |ui| {
                if let Some(send) = self.compose(ui) {
                    action = send;
                }
            });
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(CENTER_BG))
            .show_inside(ui, |ui| {
                self.header(ui);
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .stick_to_bottom(true)
                    .show(ui, |ui| {
                        if let Some(next) = self.messages(ui) {
                            action = next;
                        }
                    });
            });
        action
    }

    fn sidebar(&self, ui: &mut Ui) -> ChatAction {
        let mut action = ChatAction::None;
        let team = if self.account.teams.len() > 1 {
            format!("{}  ›", self.account.team.display_name)
        } else {
            self.account.team.display_name.clone()
        };
        let team_response = ui.add(
            egui::Label::new(RichText::new(team).color(SIDEBAR_TEXT).strong().size(16.0))
                .sense(Sense::click()),
        );
        if self.account.teams.len() > 1 && team_response.clicked() {
            action = ChatAction::SwitchTeam;
        }
        ui.add_space(8.0);
        let list_height = (ui.available_height() - 36.0).max(0.0);
        ui.allocate_ui_with_layout(
            Vec2::new(ui.available_width(), list_height),
            egui::Layout::top_down(egui::Align::Min),
            |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
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
                                if room.mentions > 0 {
                                    label = format!("{label}  {}", room.mentions);
                                } else if room.unread && !selected {
                                    label = format!("• {label}");
                                }
                                let color = if selected || room.mentions > 0 {
                                    GOLD
                                } else {
                                    SIDEBAR_TEXT
                                };
                                let response = ui.add(
                                    egui::Label::new(RichText::new(label).color(color).size(14.0))
                                        .sense(Sense::click()),
                                );
                                if response.clicked() {
                                    action = ChatAction::Open(room.id.clone());
                                }
                            }
                            ui.add_space(8.0);
                        }
                    });
            },
        );
        self.link_status(ui);
        action
    }

    fn link_status(&self, ui: &mut Ui) {
        let (color, label) = match self.link {
            Link::Live if self.updated => (Color32::from_rgb(0x3d, 0xa8, 0x63), "Updated"),
            Link::Live => (Color32::from_rgb(0x3d, 0xa8, 0x63), "Live"),
            Link::Connecting => (GOLD, "Connecting"),
            Link::Reconnecting => (GOLD, "Reconnecting"),
            Link::Offline => (MUTED, "Offline"),
        };
        ui.horizontal(|ui| {
            let (rect, _) = ui.allocate_exact_size(Vec2::splat(10.0), Sense::hover());
            ui.painter().circle_filled(rect.center(), 4.0, color);
            ui.label(RichText::new(label).color(SIDEBAR_TEXT).size(12.0));
        });
    }

    fn header(&self, ui: &mut Ui) {
        let room = self.account.sidebar.room(&self.selected);
        let title = room
            .map(|room| room.display_name.as_str())
            .unwrap_or("Channel");
        let prefix = match room.map(|room| room.kind) {
            Some(RoomKind::Direct) => "",
            _ => "# ",
        };
        egui::Frame::NONE
            .fill(CENTER_BG)
            .inner_margin(egui::Margin::symmetric(16, 12))
            .show(ui, |ui| {
                ui.label(
                    RichText::new(format!("{prefix}{title}"))
                        .strong()
                        .size(18.0)
                        .color(Color32::from_rgb(0x1b, 0x1d, 0x22)),
                );
                ui.separator();
            });
    }

    fn messages(&mut self, ui: &mut Ui) -> Option<ChatAction> {
        let mut action = None;
        let me = self.account.me.id.clone();
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
                    let body = ui.add(
                        egui::Label::new(
                            RichText::new(&message.body)
                                .size(15.0)
                                .color(Color32::from_rgb(0x3d, 0x3c, 0x40)),
                        )
                        .sense(Sense::click()),
                    );
                    if message.user_id == me {
                        let id = message.id.clone();
                        let source = message.body_source.clone();
                        body.context_menu(|ui| {
                            if ui.button("Edit").clicked() {
                                self.editing = Some(id.clone());
                                self.draft = source;
                                ui.close();
                            }
                            if ui.button("Delete").clicked() {
                                action = Some(ChatAction::Delete { post_id: id });
                                ui.close();
                            }
                        });
                    }
                });
            });
            ui.add_space(10.0);
        }
        action
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

    fn compose(&mut self, ui: &mut Ui) -> Option<ChatAction> {
        let mut action = None;
        let title = self
            .account
            .sidebar
            .room(&self.selected)
            .map(|room| room.display_name.as_str())
            .unwrap_or("channel");
        let hint = if self.editing.is_some() {
            "Edit message  ·  Enter to save, Esc to cancel".to_string()
        } else {
            format!("Message {title}  ·  Enter to send")
        };
        egui::Frame::NONE
            .inner_margin(egui::Margin::symmetric(16, 8))
            .show(ui, |ui| {
                if let Some(notice) = &self.notice {
                    ui.colored_label(Color32::from_rgb(0xd5, 0x24, 0x4a), notice);
                }
                let response = ui.add(
                    egui::TextEdit::singleline(&mut self.draft)
                        .desired_width(f32::INFINITY)
                        .hint_text(hint),
                );
                let enter =
                    response.lost_focus() && ui.input(|input| input.key_pressed(egui::Key::Enter));
                let cancel = ui.input(|input| input.key_pressed(egui::Key::Escape));
                if cancel && self.editing.is_some() {
                    self.editing = None;
                    self.draft.clear();
                } else if enter {
                    let text = self.draft.trim().to_string();
                    if !text.is_empty() && !self.selected.is_empty() {
                        action = Some(ChatAction::Send {
                            channel_id: self.selected.clone(),
                            text,
                            editing: self.editing.clone(),
                        });
                        self.draft.clear();
                        self.editing = None;
                        self.notice = None;
                    }
                }
            });
        action
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

pub struct LoginForm {
    pub site: String,
    pub login_id: String,
    pub password: String,
    pub totp: String,
    pub pat: String,
    pub paste: String,
    pub providers: Vec<String>,
    pub error: Option<String>,
    pub busy: bool,
    pub need_mfa: bool,
    pub waiting_browser: bool,
}

impl LoginForm {
    pub fn new(site: String) -> Self {
        Self {
            site,
            login_id: String::new(),
            password: String::new(),
            totp: String::new(),
            pat: String::new(),
            paste: String::new(),
            providers: Vec::new(),
            error: None,
            busy: false,
            need_mfa: false,
            waiting_browser: false,
        }
    }

    pub fn show(&mut self, ui: &mut Ui) -> LoginAction {
        let mut action = LoginAction::None;
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(CENTER_BG))
            .show_inside(ui, |ui| {
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.add_space(36.0);
                        ui.label(RichText::new("ComPort").size(28.0).strong());
                        ui.label(
                            RichText::new("Sign in to your Mattermost server")
                                .color(MUTED)
                                .size(16.0),
                        );
                    });
                    ui.add_space(20.0);
                    ui.horizontal(|ui| {
                        let pad = ((ui.available_width() - 480.0) / 2.0).max(24.0);
                        ui.add_space(pad);
                        ui.vertical(|ui| {
                            ui.set_max_width(480.0);
                            ui.label("Site URL");
                            ui.add(
                                egui::TextEdit::singleline(&mut self.site)
                                    .desired_width(480.0)
                                    .hint_text("https://chat.company.com"),
                            );
                            ui.add_space(16.0);
                            let label = if self.busy {
                                "Opening browser…"
                            } else if self.waiting_browser {
                                "Waiting for the browser…"
                            } else {
                                "Sign in with browser"
                            };
                            if ui
                                .add_enabled(!self.busy, egui::Button::new(label))
                                .clicked()
                            {
                                action = LoginAction::Browser;
                            }
                            ui.add_space(8.0);
                            let help = if self.providers.is_empty() {
                                "Opens your system browser on this server's login page. Use the same SSO button you use on the web (SAML, Google, Entra ID, GitLab, OpenID). ComPort does not embed that page, and it does not need a personal access token.".to_string()
                            } else {
                                format!(
                                    "This server offers {}. Finish sign-in in the browser. ComPort does not embed that page.",
                                    self.providers.join(", ")
                                )
                            };
                            ui.label(RichText::new(help).color(MUTED).size(13.0));
                            if self.waiting_browser {
                                ui.add_space(12.0);
                                ui.label("If the browser does not return here, paste the page address");
                                ui.add(
                                    egui::TextEdit::singleline(&mut self.paste)
                                        .desired_width(480.0)
                                        .hint_text("https://…/login/desktop?server_token=…"),
                                );
                                if ui.button("Use this link").clicked() {
                                    action = LoginAction::Paste;
                                }
                                ui.add_space(6.0);
                                ui.label(
                                    RichText::new(
                                        "If a prompt offers to open Mattermost, allow it when ComPort is the handler. If the official desktop app opens instead, cancel that and paste the address.",
                                    )
                                    .color(MUTED)
                                    .size(13.0),
                                );
                            }
                            ui.add_space(16.0);
                            egui::CollapsingHeader::new("Password or token")
                                .default_open(self.need_mfa)
                                .show(ui, |ui| {
                                    ui.label("Username or email");
                                    ui.add(
                                        egui::TextEdit::singleline(&mut self.login_id)
                                            .desired_width(480.0)
                                            .hint_text("you@company.com"),
                                    );
                                    ui.add_space(8.0);
                                    ui.label("Password");
                                    ui.add(
                                        egui::TextEdit::singleline(&mut self.password)
                                            .desired_width(480.0)
                                            .password(true),
                                    );
                                    if self.need_mfa {
                                        ui.add_space(8.0);
                                        ui.label("MFA code");
                                        ui.add(
                                            egui::TextEdit::singleline(&mut self.totp)
                                                .desired_width(480.0)
                                                .hint_text("6-digit code"),
                                        );
                                    }
                                    ui.add_space(8.0);
                                    ui.label(
                                        RichText::new(
                                            "Personal access token, only if an admin has enabled them",
                                        )
                                        .color(MUTED)
                                        .size(13.0),
                                    );
                                    ui.add(
                                        egui::TextEdit::singleline(&mut self.pat)
                                            .desired_width(480.0)
                                            .password(true)
                                            .hint_text("optional"),
                                    );
                                    ui.add_space(8.0);
                                    if ui
                                        .add_enabled(!self.busy, egui::Button::new("Sign in with password"))
                                        .clicked()
                                    {
                                        action = LoginAction::Password;
                                    }
                                });
                            if let Some(error) = &self.error {
                                ui.add_space(8.0);
                                ui.colored_label(Color32::from_rgb(0xd5, 0x24, 0x4a), error);
                            }
                        });
                    });
                });
            });
        action
    }
}
