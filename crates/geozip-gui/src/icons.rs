use eframe::egui::{
    self, pos2, vec2, Align2, Color32, FontId, Painter, Pos2, Rect, Shape, Stroke, StrokeKind,
};

#[derive(Clone, Copy, PartialEq)]
pub enum Icon {
    AddFiles,
    AddFolder,
    Remove,
    Protect,
    Compress,
    Test,
    Up,
    Open,
    Extract,
    Info,
    Folder,
    File,
    Locked,
}

const FOLDER: Color32 = Color32::from_rgb(250, 204, 21);
const FOLDER_DARK: Color32 = Color32::from_rgb(202, 138, 4);
const PAPER: Color32 = Color32::from_rgb(252, 252, 252);
const PAPER_EDGE: Color32 = Color32::from_rgb(120, 130, 145);
const GREEN: Color32 = Color32::from_rgb(22, 163, 74);
const RED: Color32 = Color32::from_rgb(220, 38, 38);
const BLUE: Color32 = Color32::from_rgb(37, 99, 235);
const STEEL: Color32 = Color32::from_rgb(100, 116, 139);

struct Canvas<'a> {
    painter: &'a Painter,
    rect: Rect,
}

impl Canvas<'_> {
    fn at(&self, x: f32, y: f32) -> Pos2 {
        self.rect.min + vec2(x * self.rect.width(), y * self.rect.height())
    }

    fn len(&self, v: f32) -> f32 {
        v * self.rect.width()
    }

    fn area(&self, x0: f32, y0: f32, x1: f32, y1: f32) -> Rect {
        Rect::from_min_max(self.at(x0, y0), self.at(x1, y1))
    }

    fn polygon(&self, points: &[(f32, f32)], fill: Color32) {
        let points = points.iter().map(|&(x, y)| self.at(x, y)).collect();
        self.painter
            .add(Shape::convex_polygon(points, fill, Stroke::NONE));
    }

    fn folder(&self, x0: f32, y0: f32, x1: f32, y1: f32) {
        let w = x1 - x0;
        let h = y1 - y0;
        self.painter.rect_filled(
            self.area(x0, y0, x0 + w * 0.45, y0 + h * 0.4),
            self.len(0.04),
            FOLDER_DARK,
        );
        self.painter
            .rect_filled(self.area(x0, y0 + h * 0.18, x1, y1), self.len(0.05), FOLDER_DARK);
        self.painter
            .rect_filled(self.area(x0, y0 + h * 0.3, x1, y1), self.len(0.05), FOLDER);
    }

    fn page(&self, x0: f32, y0: f32, x1: f32, y1: f32) {
        let area = self.area(x0, y0, x1, y1);
        self.painter.rect_filled(area, self.len(0.03), PAPER);
        self.painter.rect_stroke(
            area,
            self.len(0.03),
            Stroke::new(1.0, PAPER_EDGE),
            StrokeKind::Inside,
        );
        let w = x1 - x0;
        let h = y1 - y0;
        for i in 0..3 {
            let y = y0 + h * (0.32 + 0.2 * i as f32);
            self.painter.line_segment(
                [self.at(x0 + w * 0.2, y), self.at(x1 - w * 0.2, y)],
                Stroke::new(1.0, PAPER_EDGE.gamma_multiply(0.6)),
            );
        }
    }

    fn badge(&self, x: f32, y: f32, color: Color32, plus: bool) {
        let center = self.at(x, y);
        let r = self.len(0.2);
        self.painter.circle_filled(center, r, color);
        self.painter
            .circle_stroke(center, r, Stroke::new(1.0, Color32::WHITE));
        let arm = r * 0.55;
        let stroke = Stroke::new(self.len(0.07).max(1.5), Color32::WHITE);
        self.painter
            .line_segment([center - vec2(arm, 0.0), center + vec2(arm, 0.0)], stroke);
        if plus {
            self.painter
                .line_segment([center - vec2(0.0, arm), center + vec2(0.0, arm)], stroke);
        }
    }

    fn padlock(&self, x0: f32, y0: f32, x1: f32, y1: f32, color: Color32, open: bool) {
        let w = x1 - x0;
        let h = y1 - y0;
        let shackle_center = self.at(x0 + w * if open { 0.72 } else { 0.5 }, y0 + h * 0.36);
        self.painter.circle_stroke(
            shackle_center,
            self.len(w * 0.27),
            Stroke::new(self.len(w * 0.13).max(1.5), STEEL),
        );
        let body = self.area(x0, y0 + h * 0.42, x1, y1);
        self.painter.rect_filled(body, self.len(0.05), color);
        let hole = body.center() - vec2(0.0, body.height() * 0.08);
        self.painter
            .circle_filled(hole, body.width() * 0.11, Color32::WHITE);
        self.painter.line_segment(
            [hole, hole + vec2(0.0, body.height() * 0.3)],
            Stroke::new(body.width() * 0.09, Color32::WHITE),
        );
    }

    fn arrow_down(&self, x: f32, y0: f32, y1: f32, color: Color32) {
        let half = 0.07;
        self.painter
            .rect_filled(self.area(x - half, y0, x + half, y1 - 0.14), 0.0, color);
        self.polygon(&[(x - 0.19, y1 - 0.16), (x + 0.19, y1 - 0.16), (x, y1)], color);
    }
}

pub fn paint(painter: &Painter, rect: Rect, icon: Icon) {
    let c = Canvas { painter, rect };
    match icon {
        Icon::Folder => c.folder(0.08, 0.14, 0.92, 0.86),
        Icon::File => c.page(0.2, 0.08, 0.8, 0.92),
        Icon::AddFiles => {
            c.page(0.14, 0.06, 0.7, 0.86);
            c.badge(0.74, 0.74, GREEN, true);
        }
        Icon::AddFolder => {
            c.folder(0.04, 0.14, 0.84, 0.8);
            c.badge(0.76, 0.76, GREEN, true);
        }
        Icon::Remove => {
            c.page(0.14, 0.06, 0.7, 0.86);
            c.badge(0.74, 0.74, RED, false);
        }
        Icon::Compress => {
            c.folder(0.08, 0.14, 0.92, 0.86);
            let teeth = Stroke::new(c.len(0.07).max(1.5), STEEL);
            for step in 0..5 {
                let y = 0.3 + 0.11 * step as f32;
                let (x0, x1) = if step % 2 == 0 { (0.42, 0.5) } else { (0.5, 0.58) };
                painter.line_segment([c.at(x0, y), c.at(x1, y)], teeth);
            }
        }
        Icon::Test => {
            c.page(0.14, 0.06, 0.7, 0.86);
            let center = c.at(0.72, 0.72);
            painter.circle_filled(center, c.len(0.22), GREEN);
            painter.circle_stroke(center, c.len(0.22), Stroke::new(1.0, Color32::WHITE));
            let tick = vec![c.at(0.61, 0.72), c.at(0.69, 0.8), c.at(0.83, 0.64)];
            painter.add(Shape::line(
                tick,
                Stroke::new(c.len(0.07).max(1.5), Color32::WHITE),
            ));
        }
        Icon::Up => {
            c.polygon(&[(0.5, 0.12), (0.86, 0.52), (0.14, 0.52)], STEEL);
            painter.rect_filled(c.area(0.38, 0.5, 0.62, 0.88), 0.0, STEEL);
        }
        Icon::Protect => {
            c.folder(0.02, 0.2, 0.78, 0.84);
            c.padlock(0.5, 0.38, 0.96, 0.98, BLUE, false);
        }
        Icon::Open => {
            c.folder(0.06, 0.16, 0.94, 0.84);
            c.polygon(
                &[(0.18, 0.5), (1.0, 0.5), (0.86, 0.84), (0.06, 0.84)],
                FOLDER.gamma_multiply(1.0),
            );
            painter.line_segment([c.at(0.18, 0.5), c.at(1.0, 0.5)], Stroke::new(1.0, FOLDER_DARK));
        }
        Icon::Extract => {
            c.folder(0.06, 0.4, 0.94, 0.96);
            c.arrow_down(0.5, 0.02, 0.62, GREEN);
        }
        Icon::Info => {
            let center = rect.center();
            let r = rect.width() * 0.42;
            painter.circle_filled(center, r, BLUE);
            painter.text(
                center,
                Align2::CENTER_CENTER,
                "i",
                FontId::proportional(rect.height() * 0.62),
                Color32::WHITE,
            );
        }
        Icon::Locked => c.padlock(0.22, 0.06, 0.78, 0.94, BLUE, false),
    }
}

pub fn small(ui: &mut egui::Ui, icon: Icon) {
    let (rect, _) = ui.allocate_exact_size(vec2(16.0, 16.0), egui::Sense::hover());
    paint(ui.painter(), rect, icon);
}

pub fn info(ui: &mut egui::Ui, text: &str) {
    let (rect, response) = ui.allocate_exact_size(vec2(15.0, 15.0), egui::Sense::hover());
    let painter = ui.painter();
    let color = if response.hovered() { BLUE } else { STEEL };
    painter.circle_stroke(rect.center(), 6.5, Stroke::new(1.2, color));
    painter.text(
        rect.center() + vec2(0.0, 0.5),
        Align2::CENTER_CENTER,
        "i",
        FontId::proportional(11.0),
        color,
    );
    response.on_hover_ui(|ui| {
        ui.set_max_width(340.0);
        ui.label(text);
    });
}

pub fn tool_button(ui: &mut egui::Ui, icon: Icon, label: &str, enabled: bool) -> bool {
    let size = vec2(78.0, 62.0);
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(size, sense);
    let painter = ui.painter();
    if enabled && response.hovered() {
        let fill = if response.is_pointer_button_down_on() {
            Color32::from_rgb(196, 218, 245)
        } else {
            Color32::from_rgb(222, 235, 250)
        };
        painter.rect_filled(rect, 3.0, fill);
        painter.rect_stroke(
            rect,
            3.0,
            Stroke::new(1.0, Color32::from_rgb(120, 170, 230)),
            StrokeKind::Inside,
        );
    }
    let icon_rect = Rect::from_center_size(pos2(rect.center().x, rect.top() + 23.0), vec2(34.0, 34.0));
    paint(painter, icon_rect, icon);
    if !enabled {
        painter.rect_filled(
            icon_rect.expand(2.0),
            0.0,
            ui.visuals().panel_fill.gamma_multiply(0.72),
        );
    }
    let color = if enabled {
        ui.visuals().text_color()
    } else {
        ui.visuals().weak_text_color()
    };
    painter.text(
        pos2(rect.center().x, rect.bottom() - 9.0),
        Align2::CENTER_CENTER,
        label,
        FontId::proportional(11.5),
        color,
    );
    enabled && response.clicked()
}

pub fn connection(ui: &mut egui::Ui, online: bool) {
    let color = if online { GREEN } else { RED };
    let (rect, _) = ui.allocate_exact_size(vec2(22.0, 18.0), egui::Sense::hover());
    let painter = ui.painter();
    let origin = pos2(rect.center().x, rect.bottom() - 3.0);
    painter.circle_filled(origin, 2.0, color);
    for radius in [5.5, 9.5, 13.5] {
        let arc: Vec<Pos2> = (0..=12)
            .map(|step| {
                let angle = (225.0 + 7.5 * step as f32).to_radians();
                origin + vec2(radius * angle.cos(), radius * angle.sin())
            })
            .collect();
        painter.add(Shape::line(arc, Stroke::new(2.0, color)));
    }
    if !online {
        let slash = [
            rect.left_top() + vec2(3.0, 1.0),
            rect.right_bottom() - vec2(3.0, 1.0),
        ];
        painter.line_segment(slash, Stroke::new(4.5, ui.visuals().panel_fill));
        painter.line_segment(slash, Stroke::new(2.0, color));
    }
}

pub fn small_button(ui: &mut egui::Ui, icon: Icon, enabled: bool) -> bool {
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, response) = ui.allocate_exact_size(vec2(20.0, 20.0), sense);
    let painter = ui.painter();
    if enabled && response.hovered() {
        painter.rect_filled(rect, 3.0, Color32::from_rgb(222, 235, 250));
    }
    paint(painter, rect.shrink(2.0), icon);
    if !enabled {
        painter.rect_filled(rect, 0.0, ui.visuals().panel_fill.gamma_multiply(0.72));
    }
    enabled && response.clicked()
}

pub fn sort_arrow(ui: &mut egui::Ui, ascending: bool) {
    let (rect, _) = ui.allocate_exact_size(vec2(9.0, 9.0), egui::Sense::hover());
    let (tip, base) = if ascending {
        (rect.top() + 2.0, rect.bottom() - 2.0)
    } else {
        (rect.bottom() - 2.0, rect.top() + 2.0)
    };
    let points = vec![
        pos2(rect.center().x, tip),
        pos2(rect.right() - 1.0, base),
        pos2(rect.left() + 1.0, base),
    ];
    ui.painter()
        .add(Shape::convex_polygon(points, STEEL, Stroke::NONE));
}
