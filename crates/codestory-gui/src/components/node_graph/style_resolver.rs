use codestory_core::{EdgeKind, NodeKind};
use eframe::egui::Color32;

use crate::settings::ThemeMode;

#[derive(Clone, Copy)]
pub struct GraphPalette {
    pub background: Color32,
    pub node_default_fill: Color32,
    pub node_default_border: Color32,
    pub node_default_text: Color32,
    pub node_default_icon: Color32,
    pub node_hatching: Color32,
    pub node_section_fill: Color32,
    pub node_section_border: Color32,
    pub section_label_fill: Color32,
    pub section_label_text: Color32,
    pub member_text_light: Color32,
    pub member_text_dark: Color32,
    pub shadow: Color32,
    pub type_fill: Color32,
    pub function_fill: Color32,
    pub variable_fill: Color32,
    pub namespace_fill: Color32,
    pub edge_default: Color32,
    pub edge_type_use: Color32,
    pub edge_inheritance: Color32,
    pub edge_use: Color32,
    pub edge_call: Color32,
    pub edge_override: Color32,
    pub edge_type_argument: Color32,
    pub edge_include: Color32,
    pub edge_bundled: Color32,
    pub minimap_background: Color32,
    pub minimap_border: Color32,
    pub minimap_node: Color32,
    pub legend_background: Color32,
}

impl GraphPalette {
    pub fn bright() -> Self {
        Self {
            background: hex_color("#FFFFFF"),
            node_default_fill: hex_color("#D6D6D6"),
            node_default_border: hex_color("#3C3C3C"),
            node_default_text: hex_color("#000000"),
            node_default_icon: hex_color("#000000"),
            node_hatching: hex_color("#F0F0F0"),
            node_section_fill: hex_color("#FFFFFF"),
            node_section_border: hex_color("#CFCFCF"),
            section_label_fill: hex_color("#F2F2F2"),
            section_label_text: hex_color("#000000"),
            member_text_light: hex_color("#FFFFFF"),
            member_text_dark: hex_color("#000000"),
            shadow: Color32::from_rgba_unmultiplied(0, 0, 0, 24),
            type_fill: hex_color("#D6D6D6"),
            function_fill: hex_color("#F4D07D"),
            variable_fill: hex_color("#81C1E3"),
            namespace_fill: hex_color("#EFC5C5"),
            edge_default: hex_color("#878787"),
            edge_type_use: hex_color("#878787"),
            edge_inheritance: hex_color("#878787"),
            edge_use: hex_color("#4B9FC4"),
            edge_call: hex_color("#F4BC3D"),
            edge_override: hex_color("#A37ACC"),
            edge_type_argument: hex_color("#CF6B7C"),
            edge_include: hex_color("#719660"),
            edge_bundled: hex_color("#CCCCCC"),
            minimap_background: hex_color("#EDEDED"),
            minimap_border: hex_color("#BEBEBE"),
            minimap_node: Color32::from_rgba_unmultiplied(0, 0, 0, 40),
            legend_background: hex_color("#F7F7F7"),
        }
    }

    pub fn dark() -> Self {
        Self {
            background: hex_color("#272728"),
            node_default_fill: hex_color("#5A5A5A"),
            node_default_border: hex_color("#C3C3C3"),
            node_default_text: hex_color("#F7F7F7"),
            node_default_icon: hex_color("#F7F7F7"),
            node_hatching: hex_color("#3D3D3D"),
            node_section_fill: hex_color("#3A3A3A"),
            node_section_border: hex_color("#5C5C5C"),
            section_label_fill: hex_color("#4A4A4A"),
            section_label_text: hex_color("#F7F7F7"),
            member_text_light: hex_color("#F7F7F7"),
            member_text_dark: hex_color("#000000"),
            shadow: Color32::from_rgba_unmultiplied(0, 0, 0, 60),
            type_fill: hex_color("#5A5A5A"),
            function_fill: hex_color("#7A681F"),
            variable_fill: hex_color("#21516B"),
            namespace_fill: hex_color("#78282D"),
            edge_default: hex_color("#A0A0A0"),
            edge_type_use: hex_color("#A0A0A0"),
            edge_inheritance: hex_color("#A0A0A0"),
            edge_use: hex_color("#4B9FC4"),
            edge_call: hex_color("#9C8528"),
            edge_override: hex_color("#A37ACC"),
            edge_type_argument: hex_color("#CF6B7C"),
            edge_include: hex_color("#719660"),
            edge_bundled: hex_color("#666666"),
            minimap_background: hex_color("#2F2F30"),
            minimap_border: hex_color("#5C5C5C"),
            minimap_node: Color32::from_rgba_unmultiplied(255, 255, 255, 30),
            legend_background: hex_color("#2F2F30"),
        }
    }

    pub fn from_theme_mode(mode: ThemeMode) -> Self {
        match mode {
            ThemeMode::Bright | ThemeMode::Latte => Self::bright(),
            ThemeMode::Dark | ThemeMode::Frappe | ThemeMode::Macchiato | ThemeMode::Mocha => {
                Self::dark()
            }
        }
    }
}

#[derive(Clone, Copy)]
pub struct StyleResolver {
    palette: GraphPalette,
}

impl StyleResolver {
    pub fn new(mode: ThemeMode) -> Self {
        Self {
            palette: GraphPalette::from_theme_mode(mode),
        }
    }

    pub fn set_theme_mode(&mut self, mode: ThemeMode) {
        self.palette = GraphPalette::from_theme_mode(mode);
    }

    pub fn palette(&self) -> GraphPalette {
        self.palette
    }

    pub fn resolve_node_color(&self, kind: NodeKind) -> Color32 {
        match kind {
            NodeKind::FUNCTION | NodeKind::METHOD | NodeKind::MACRO => self.palette.function_fill,
            NodeKind::VARIABLE | NodeKind::FIELD | NodeKind::GLOBAL_VARIABLE => {
                self.palette.variable_fill
            }
            NodeKind::NAMESPACE | NodeKind::MODULE | NodeKind::PACKAGE => {
                self.palette.namespace_fill
            }
            _ => self.palette.type_fill,
        }
    }

    pub fn resolve_text_color(&self, bg_color: Color32) -> Color32 {
        if is_light(bg_color) {
            self.palette.member_text_dark
        } else {
            self.palette.member_text_light
        }
    }

    pub fn resolve_edge_color(&self, kind: EdgeKind) -> Color32 {
        match kind {
            EdgeKind::CALL => self.palette.edge_call,
            EdgeKind::OVERRIDE => self.palette.edge_override,
            EdgeKind::INHERITANCE => self.palette.edge_inheritance,
            EdgeKind::TYPE_USAGE | EdgeKind::TYPE_ARGUMENT => self.palette.edge_type_use,
            EdgeKind::USAGE | EdgeKind::MACRO_USAGE => self.palette.edge_use,
            EdgeKind::IMPORT | EdgeKind::INCLUDE => self.palette.edge_include,
            EdgeKind::TEMPLATE_SPECIALIZATION => self.palette.edge_type_argument,
            _ => self.palette.edge_default,
        }
    }

    pub fn resolve_icon_color(&self, _kind: NodeKind) -> Color32 {
        self.palette.node_default_icon
    }

    pub fn resolve_bundled_edge_color(&self) -> Color32 {
        self.palette.edge_bundled
    }

    pub fn resolve_outgoing_edge_indicator_color(&self) -> Color32 {
        self.palette.edge_default
    }

    pub fn edge_width_scale(&self) -> f32 {
        0.9
    }
}

fn is_light(color: Color32) -> bool {
    let (r, g, b, _) = color.to_tuple();
    let luminance = 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32;
    luminance > 140.0
}

fn hex_color(hex: &str) -> Color32 {
    let hex = hex.trim_start_matches('#');
    if hex.len() == 6 {
        let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(0);
        let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(0);
        let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(0);
        Color32::from_rgb(r, g, b)
    } else if hex.len() == 8 {
        let a = u8::from_str_radix(&hex[0..2], 16).unwrap_or(255);
        let r = u8::from_str_radix(&hex[2..4], 16).unwrap_or(0);
        let g = u8::from_str_radix(&hex[4..6], 16).unwrap_or(0);
        let b = u8::from_str_radix(&hex[6..8], 16).unwrap_or(0);
        Color32::from_rgba_unmultiplied(r, g, b, a)
    } else {
        Color32::WHITE
    }
}
