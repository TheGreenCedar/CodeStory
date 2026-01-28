use codestory_events::TooltipInfo;
use eframe::egui;

pub struct TooltipManager {
    pub info: Option<TooltipInfo>,
    pub position: Option<egui::Pos2>,
}

impl TooltipManager {
    pub fn new() -> Self {
        Self {
            info: None,
            position: None,
        }
    }

    pub fn show(&mut self, info: TooltipInfo, pos: egui::Pos2) {
        self.info = Some(info);
        self.position = Some(pos);
    }

    pub fn hide(&mut self) {
        self.info = None;
        self.position = None;
    }

    pub fn ui(&self, ctx: &egui::Context) {
        if let (Some(info), Some(pos)) = (&self.info, self.position) {
            egui::Area::new("tooltip_area".into())
                .fixed_pos(pos)
                .order(egui::Order::Tooltip)
                .show(ctx, |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.label(egui::RichText::new(&info.title).strong());
                        ui.separator();
                        ui.label(&info.description);
                    });
                });
        }
    }
}
