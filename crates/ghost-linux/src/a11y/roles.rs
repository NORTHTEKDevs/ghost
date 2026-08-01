//! Role vocabulary.
//!
//! Ghost's role names (`button`, `edit`, `checkbox`, ...) are part of its public
//! MCP contract: agents send `role="button"` and expect the same meaning on any
//! platform. To guarantee zero vocabulary drift, this module maps AT-SPI roles
//! onto **the same numeric id space the Windows engine uses** (UIA control-type
//! ids, 50000+), and then reuses an identical `role_id_to_name` table.
//!
//! The practical consequence: `patterns::is_editable_role`, `role_alias_matches`
//! and every role string an agent sees are the same on Linux and Windows.

#[cfg(target_os = "linux")]
use atspi::Role;

/// Identical to `ghost_core::uia::element::role_id_to_name`. Kept in the same
/// numeric space deliberately -- see the module docs.
pub fn role_id_to_name(id: u32) -> &'static str {
    match id {
        50000 => "button",
        50001 => "calendar",
        50002 => "checkbox",
        50003 => "combobox",
        50004 => "edit",
        50005 => "hyperlink",
        50006 => "image",
        50007 => "listitem",
        50008 => "list",
        50009 => "menu",
        50010 => "menubar",
        50011 => "menuitem",
        50012 => "progressbar",
        50013 => "radiobutton",
        50014 => "scrollbar",
        50015 => "slider",
        50016 => "spinner",
        50017 => "statusbar",
        50018 => "tab",
        50019 => "tabitem",
        50020 => "text",
        50021 => "toolbar",
        50022 => "tooltip",
        50023 => "tree",
        50024 => "treeitem",
        50025 => "custom",
        50026 => "group",
        50027 => "thumb",
        50028 => "datagrid",
        50029 => "dataitem",
        50030 => "document",
        50031 => "splitbutton",
        50032 => "window",
        50033 => "pane",
        50034 => "header",
        50035 => "headeritem",
        50036 => "table",
        50037 => "titlebar",
        50038 => "separator",
        _ => "unknown",
    }
}

/// Roles included in `describe_screen` output. Identical to the Windows engine's
/// `INTERACTIVE_ROLES`, including `splitbutton`/`dataitem`.
pub const INTERACTIVE_ROLES: &[&str] = &[
    "button", "edit", "checkbox", "combobox", "menu", "menuitem",
    "tab", "tabitem", "list", "listitem", "toolbar", "radiobutton",
    "hyperlink", "treeitem", "document",
    "splitbutton", "dataitem",
];

/// Alias rules, identical to `ghost_core::uia::tree::role_alias_matches`.
///
/// The `edit -> document` alias is load-bearing on both platforms: rich text
/// surfaces report themselves as documents, but agents ask for `edit`.
pub fn role_alias_matches(searched: &str, el_role: &str) -> bool {
    match searched {
        "tab" => el_role == "tabitem",
        "list" => el_role == "listitem",
        "edit" => el_role == "document",
        _ => false,
    }
}

/// Map an AT-SPI role onto Ghost's numeric role id.
///
/// Anything without a meaningful Ghost equivalent becomes `custom` (50025)
/// rather than being dropped, so an agent can still see and target it.
#[cfg(target_os = "linux")]
pub fn atspi_role_to_id(role: Role) -> u32 {
    match role {
        // Buttons and button-likes. Note the AT-SPI variant is `Button`, not
        // `PushButton` (the spec name); the crate diverges here.
        Role::Button => 50000,
        Role::ToggleButton => 50000,
        Role::PushButtonMenu => 50031,

        // Selection controls
        Role::CheckBox => 50002,
        Role::CheckMenuItem => 50002,
        Role::RadioButton => 50013,
        Role::RadioMenuItem => 50013,
        Role::ComboBox => 50003,

        // Text entry / display
        Role::Entry => 50004,
        Role::PasswordText => 50004,
        Role::Text => 50020,
        Role::Label => 50020,
        Role::Static => 50020,
        Role::Heading => 50020,
        Role::Paragraph => 50020,
        Role::Caption => 50020,

        // Documents (the `edit` alias target)
        Role::DocumentText => 50030,
        Role::DocumentWeb => 50030,
        Role::DocumentFrame => 50030,
        Role::DocumentEmail => 50030,
        Role::DocumentSpreadsheet => 50030,
        Role::DocumentPresentation => 50030,
        Role::Terminal => 50030,

        // Links and media
        Role::Link => 50005,
        Role::Image => 50006,
        Role::Icon => 50006,

        // Lists and trees
        Role::List => 50008,
        Role::ListBox => 50008,
        Role::ListItem => 50007,
        Role::Tree => 50023,
        Role::TreeItem => 50024,
        Role::TreeTable => 50023,

        // Menus
        Role::Menu => 50009,
        Role::MenuBar => 50010,
        Role::PopupMenu => 50009,
        Role::MenuItem => 50011,

        // Indicators
        Role::ProgressBar => 50012,
        Role::LevelBar => 50012,
        Role::ScrollBar => 50014,
        Role::Slider => 50015,
        Role::SpinButton => 50016,
        Role::StatusBar => 50017,

        // Tabs
        Role::PageTab => 50019,
        Role::PageTabList => 50018,

        // Bars
        Role::ToolBar => 50021,
        Role::ToolTip => 50022,

        // Tables
        Role::Table => 50036,
        Role::TableCell => 50029,
        Role::TableRow => 50029,
        Role::TableColumnHeader => 50035,
        Role::TableRowHeader => 50035,
        Role::ColumnHeader => 50035,
        Role::RowHeader => 50035,

        // Containers and windows
        Role::Frame => 50032,
        Role::Window => 50032,
        Role::Dialog => 50032,
        Role::Alert => 50032,
        Role::Panel => 50033,
        Role::Filler => 50033,
        Role::Viewport => 50033,
        Role::ScrollPane => 50033,
        Role::SplitPane => 50033,
        Role::LayeredPane => 50033,
        Role::RootPane => 50033,
        Role::Grouping => 50026,
        Role::Section => 50026,
        Role::Form => 50026,

        // Misc
        Role::Separator => 50038,
        Role::Calendar => 50001,
        Role::DateEditor => 50001,

        _ => 50025,
    }
}

/// Convenience: AT-SPI role straight to Ghost's role name.
#[cfg(target_os = "linux")]
pub fn atspi_role_name(role: Role) -> &'static str {
    role_id_to_name(atspi_role_to_id(role))
}

/// True when this role should appear in `describe_screen`.
pub fn is_interactive_role(role_name: &str) -> bool {
    INTERACTIVE_ROLES.contains(&role_name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn role_ids_match_the_windows_engine() {
        // These exact ids are asserted by ghost-core's own tests. If either side
        // changes, the platforms have diverged and agents would see different
        // role names for the same widget.
        assert_eq!(role_id_to_name(50000), "button");
        assert_eq!(role_id_to_name(50004), "edit");
        assert_eq!(role_id_to_name(50002), "checkbox");
        assert_eq!(role_id_to_name(50030), "document");
        assert_eq!(role_id_to_name(50031), "splitbutton");
        assert_eq!(role_id_to_name(50029), "dataitem");
    }

    #[test]
    fn alias_rules_match_the_windows_engine() {
        assert!(role_alias_matches("edit", "document"));
        assert!(role_alias_matches("tab", "tabitem"));
        assert!(role_alias_matches("list", "listitem"));
        assert!(!role_alias_matches("button", "edit"));
        assert!(!role_alias_matches("edit", "button"));
    }

    #[test]
    fn interactive_roles_include_the_windows_additions() {
        assert!(is_interactive_role("splitbutton"));
        assert!(is_interactive_role("dataitem"));
        assert!(!is_interactive_role("separator"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn gtk_entry_is_an_edit() {
        assert_eq!(atspi_role_name(Role::Entry), "edit");
        assert_eq!(atspi_role_name(Role::PasswordText), "edit");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn buttons_map_to_button() {
        assert_eq!(atspi_role_name(Role::Button), "button");
        assert_eq!(atspi_role_name(Role::ToggleButton), "button");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn text_surfaces_map_to_document_so_the_edit_alias_reaches_them() {
        // Mirrors the Windows finding that a rich text area is a Document and
        // the edit->document alias is required to target it.
        assert_eq!(atspi_role_name(Role::DocumentText), "document");
        assert_eq!(atspi_role_name(Role::Terminal), "document");
        assert!(role_alias_matches("edit", atspi_role_name(Role::DocumentText)));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn page_tab_is_tabitem_not_tab() {
        // A single tab is `tabitem`; the tab strip is `tab`. Getting this
        // backwards breaks the `tab -> tabitem` alias.
        assert_eq!(atspi_role_name(Role::PageTab), "tabitem");
        assert_eq!(atspi_role_name(Role::PageTabList), "tab");
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn unknown_roles_degrade_to_custom_not_dropped() {
        assert_eq!(atspi_role_name(Role::Invalid), "custom");
    }
}
