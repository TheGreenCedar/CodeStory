use codestory_core::{EdgeKind, NodeKind};
use eframe::egui::Color32;

/// Resolves semantic colors based on the current Catppuccin theme.
///
/// This component bridges the gap between semantic types (NodeKind, EdgeKind)
/// and the purely visual theme colors. It ensures that standard Sourcetrail
/// color meanings are preserved (Class=Gray/Blue, Function=Yellow, etc.)
/// while adapting to the specific palette of the active theme (Latte/Mocha/etc.).
#[derive(Clone, Copy)]
pub struct StyleResolver {
    pub theme: catppuccin_egui::Theme,
}

impl StyleResolver {
    pub fn new(theme: catppuccin_egui::Theme) -> Self {
        Self { theme }
    }

    /// Update the internal theme
    pub fn set_theme(&mut self, theme: catppuccin_egui::Theme) {
        self.theme = theme;
    }

    /// Get the fill color for a node kind.
    ///
    /// Mappings:
    /// - Classes/Types -> Blue/Sapphire (Structural)
    /// - Functions/Methods -> Yellow/Peach (Executable)
    /// - Variables/Fields -> Text/Subtext (Data)
    /// - Files/Modules -> Mauve/Pink (Organizational)
    pub fn resolve_node_color(&self, kind: NodeKind) -> Color32 {
        match kind {
            // Types - Blueish tones
            NodeKind::CLASS => self.theme.blue,
            NodeKind::STRUCT => self.theme.teal,
            NodeKind::INTERFACE => self.theme.sky,
            NodeKind::UNION => self.theme.sapphire,
            
            // Callables - Warm tones
            NodeKind::FUNCTION => self.theme.yellow,
            NodeKind::METHOD => self.theme.peach,
            NodeKind::MACRO => self.theme.maroon,
            
            // Containers/Files - Purple/Pink tones
            NodeKind::MODULE | NodeKind::NAMESPACE => self.theme.mauve,
            NodeKind::PACKAGE => self.theme.pink,
            NodeKind::FILE => self.theme.lavender,
            
            // Data - Neutral/Text tones
            NodeKind::VARIABLE 
            | NodeKind::FIELD 
            | NodeKind::GLOBAL_VARIABLE => self.theme.text,
            
            NodeKind::CONSTANT 
            | NodeKind::ENUM_CONSTANT => self.theme.subtext0,
            
            // Bundle/Other
            _ => self.theme.overlay1,
        }
    }

    /// Get the text color that contrasts best with the given background color.
    ///
    /// For dark themes (Mocha), we generally want light text on dark backgrounds.
    /// However, if the node background is very bright (e.g. Yellow function nodes),
    /// we might need dark text even in a dark theme.
    ///
    /// Catppuccin colors are generally desaturated enough that Crust/Base (dark) text
    /// works well on them in light mode, and Text (light) works well in dark mode.
    /// But specific bright colors like Yellow might need attention.
    ///
    /// **Validates: Requirements 12.4, 12.5**
    pub fn resolve_text_color(&self, bg_color: Color32) -> Color32 {
        // Simple luminance check or hardcoded preferences based on known palette
        // For Catppuccin, the 'text' color is designed to contrast with 'base'.
        // But for colored headers:
        
        // If the background is one of the bright accent colors (Yellow, Peach, Blue),
        // we might want dark text (Crust/Base) for better readability, 
        // especially if the theme is effectively "Dark".
        
        // Let's use a heuristic: if the theme is "dark" (Mocha/Macchiato/Frappe), 
        // the background 'base' is dark. 
        // If we put text on top of 'Yellow' (which is bright), we probably want Black/Crust text.
        
        // However, `catppuccin_egui::Theme` struct doesn't strictly say if it's light/dark 
        // without checking values.
        // We can check the luminance of the surface0 color.
        
        if self.is_dark_theme() {
            // In dark mode:
            // Backgrounds like Yellow, Peach, Green are bright -> Use Dark Text (Crust)
            // Backgrounds like Overlay, Surface are dark -> Use Light Text (Text)
            
            // We can approximate by checking if bg_color is an "Accent" color
            if bg_color == self.theme.yellow 
                || bg_color == self.theme.peach 
                || bg_color == self.theme.green 
                || bg_color == self.theme.teal
                || bg_color == self.theme.sky
                || bg_color == self.theme.blue
                || bg_color == self.theme.lavender
                || bg_color == self.theme.rosewater
                || bg_color == self.theme.pink
            {
                // Contrast: Black/Crust on bright accents
                self.theme.crust
            } else {
                // White/Text on dark backgrounds (Overlay, Surface, Base)
                self.theme.text
            }
        } else {
            // In light mode (Latte):
            // Backgrounds are pastel but light. Text should be Dark (Text/Base).
            // Actually in Latte, 'Text' is dark gray.
            self.theme.text
        }
    }

    /// Helper to guess if we are in a dark theme based on crust luminance
    fn is_dark_theme(&self) -> bool {
        let (r, g, b, _) = self.theme.crust.to_tuple();
        let luminance = 0.299 * r as f32 + 0.587 * g as f32 + 0.114 * b as f32;
        luminance < 128.0
    }

    /// Get the edge color for an edge kind.
    ///
    /// Mappings:
    /// - Call -> Yellow (Function -> Function)
    /// - Inheritance -> Blue (Class -> Class)
    /// - Usage -> Text/Overlay (Variable usage)
    /// - Member -> Overlay0 (Subtle structural link)
    pub fn resolve_edge_color(&self, kind: EdgeKind) -> Color32 {
        match kind {
            EdgeKind::CALL => self.theme.yellow,
            EdgeKind::INHERITANCE 
            | EdgeKind::OVERRIDE 
            | EdgeKind::TEMPLATE_SPECIALIZATION => self.theme.blue,
            
            EdgeKind::TYPE_USAGE 
            | EdgeKind::TYPE_ARGUMENT => self.theme.teal,
            
            EdgeKind::USAGE 
            | EdgeKind::MACRO_USAGE => self.theme.subtext0,
            
            EdgeKind::MEMBER => self.theme.overlay0,
            
            EdgeKind::IMPORT 
            | EdgeKind::INCLUDE => self.theme.green,
            
            EdgeKind::ANNOTATION_USAGE => self.theme.maroon,
            
            _ => self.theme.overlay1,
        }
    }
    
    /// Get the icon color (usually same as node color or slightly different)
    pub fn resolve_icon_color(&self, kind: NodeKind) -> Color32 {
        self.resolve_node_color(kind)
    }

    /// Get color for outgoing edge indicator arrow on member rows
    pub fn resolve_outgoing_edge_indicator_color(&self) -> Color32 {
        self.theme.subtext0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use catppuccin_egui::Theme;
    use egui::Color32;

    // We can't easily construct a full Theme manually without the crate's constructor if it's private fields,
    // but catppuccin_egui exposes constants like MOCHA, LATTE.
    
    #[test]
    fn test_resolve_node_color_consistency() {
        let resolver = StyleResolver::new(catppuccin_egui::MOCHA);
        
        // Class should be Blue in Mocha
        assert_eq!(resolver.resolve_node_color(NodeKind::CLASS), catppuccin_egui::MOCHA.blue);
        
        // Function should be Yellow
        assert_eq!(resolver.resolve_node_color(NodeKind::FUNCTION), catppuccin_egui::MOCHA.yellow);
    }
    
    #[test]
    fn test_text_contrast_dark_theme() {
        let resolver = StyleResolver::new(catppuccin_egui::MOCHA);
        
        // On a bright background (Yellow), text should be dark (Crust)
        let bg = catppuccin_egui::MOCHA.yellow;
        let text = resolver.resolve_text_color(bg);
        assert_eq!(text, catppuccin_egui::MOCHA.crust);
        
        // On a dark background (Overlay1), text should be light (Text)
        let bg = catppuccin_egui::MOCHA.overlay1;
        let text = resolver.resolve_text_color(bg);
        assert_eq!(text, catppuccin_egui::MOCHA.text);
    }
    
    #[test]
    fn test_text_contrast_light_theme() {
        // LATTE is the light theme
        let resolver = StyleResolver::new(catppuccin_egui::LATTE);
        
        // In light theme, text color (which is dark gray) should be used on light backgrounds
        let bg = catppuccin_egui::LATTE.blue;
        let text = resolver.resolve_text_color(bg);
        assert_eq!(text, catppuccin_egui::LATTE.text);
    }

    // Property-based tests
    #[cfg(test)]
    mod property_tests {
        use super::*;
        use proptest::prelude::*;
        
        // Strategy for NodeKind
        fn node_kind_strategy() -> impl Strategy<Value = NodeKind> {
            prop_oneof![
                Just(NodeKind::CLASS),
                Just(NodeKind::FUNCTION),
                Just(NodeKind::VARIABLE),
                Just(NodeKind::MODULE),
                Just(NodeKind::UNKNOWN),
            ]
        }
        
        proptest! {
            /// **Validates: Requirement 12.1, Property 27**
            #[test]
            fn prop_theme_color_update(kind in node_kind_strategy()) {
                let mocha_resolver = StyleResolver::new(catppuccin_egui::MOCHA);
                let latte_resolver = StyleResolver::new(catppuccin_egui::LATTE);
                
                let mocha_color = mocha_resolver.resolve_node_color(kind);
                let latte_color = latte_resolver.resolve_node_color(kind);
                
                // Colors should differ between themes (with rare exceptions if palette matches exactly for some reason)
                // In Catppuccin, Latte and Mocha are distinct enough.
                prop_assert_ne!(mocha_color, latte_color, "Colors should update when theme changes");
            }
        }
    }
}
