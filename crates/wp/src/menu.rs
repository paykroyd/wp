//! The pull-down menu bar — WordPerfect 5.1's `Alt+=` menus. Hidden by
//! default ("the screen is for writing"), shown while a menu is open, or
//! pinned on with View ▸ Menu Bar. Every item is a registry command, so the
//! menus can never reach something the palette cannot (SPEC §5.4).

use crate::commands::Cmd;

pub enum Item {
    Cmd(Cmd),
    Sep,
}

pub struct Menu {
    pub title: &'static str,
    /// The letter that opens this menu from the bar (underlined in the title).
    pub mnemonic: char,
    pub items: &'static [Item],
}

use Item::{Cmd as C, Sep};

pub static MENUS: &[Menu] = &[
    Menu {
        title: "File",
        mnemonic: 'F',
        items: &[
            C(Cmd::New),
            C(Cmd::Open),
            C(Cmd::Save),
            C(Cmd::SaveAs),
            Sep,
            C(Cmd::SaveAsDocx),
            C(Cmd::SaveAsMarkdown),
            C(Cmd::SaveAsText),
            Sep,
            C(Cmd::Warnings),
            Sep,
            C(Cmd::Exit),
        ],
    },
    Menu {
        title: "Edit",
        mnemonic: 'E',
        items: &[
            C(Cmd::Undo),
            C(Cmd::Redo),
            Sep,
            C(Cmd::Cut),
            C(Cmd::Copy),
            C(Cmd::Paste),
            C(Cmd::PastePlain),
            C(Cmd::PasteFromRing),
            Sep,
            C(Cmd::Block),
            C(Cmd::SelectAll),
            Sep,
            C(Cmd::DeleteWord),
            C(Cmd::DeleteToEndOfLine),
            C(Cmd::DeleteToStartOfLine),
            Sep,
            C(Cmd::Typeover),
        ],
    },
    Menu {
        title: "Search",
        mnemonic: 'S',
        items: &[
            C(Cmd::Find),
            C(Cmd::FindBackward),
            C(Cmd::FindNext),
            C(Cmd::FindPrev),
            C(Cmd::Replace),
            Sep,
            C(Cmd::FindRegex),
            C(Cmd::FindToggleCase),
            C(Cmd::FindToggleWord),
            C(Cmd::FindToggleRegex),
            C(Cmd::FindCode),
            Sep,
            C(Cmd::GoToPage),
            C(Cmd::GoToHeading),
            C(Cmd::GoToBookmark),
        ],
    },
    Menu {
        title: "Layout",
        mnemonic: 'L',
        items: &[
            C(Cmd::AlignLeft),
            C(Cmd::AlignCenter),
            C(Cmd::AlignRight),
            C(Cmd::AlignJustify),
            Sep,
            C(Cmd::Indent),
            C(Cmd::Outdent),
            C(Cmd::IndentLeftRight),
            C(Cmd::HangingIndent),
            C(Cmd::FirstLineIndent),
            C(Cmd::TabSet),
            Sep,
            C(Cmd::SpacingSingle),
            C(Cmd::SpacingOneHalf),
            C(Cmd::SpacingDouble),
            C(Cmd::SpaceBefore),
            C(Cmd::SpaceAfter),
            Sep,
            C(Cmd::KeepWithNext),
            C(Cmd::KeepLinesTogether),
            C(Cmd::PageBreakBefore),
            C(Cmd::WidowOrphan),
            Sep,
            C(Cmd::PageSetup),
            C(Cmd::Margins),
            C(Cmd::PaperLetter),
            C(Cmd::PaperA4),
            C(Cmd::Portrait),
            C(Cmd::Landscape),
        ],
    },
    Menu {
        title: "Font",
        mnemonic: 'O',
        items: &[
            C(Cmd::Bold),
            C(Cmd::Italic),
            C(Cmd::Underline),
            C(Cmd::DoubleUnderline),
            C(Cmd::Strikethrough),
            C(Cmd::Superscript),
            C(Cmd::Subscript),
            C(Cmd::SmallCaps),
            C(Cmd::AllCaps),
            Sep,
            C(Cmd::Font),
            C(Cmd::FontSize),
            C(Cmd::FontColor),
            C(Cmd::Highlight),
            Sep,
            C(Cmd::RemoveFormatting),
        ],
    },
    Menu {
        title: "Styles",
        mnemonic: 'Y',
        items: &[
            C(Cmd::ApplyStyle),
            Sep,
            C(Cmd::StyleNormal),
            C(Cmd::StyleHeading1),
            C(Cmd::StyleHeading2),
            C(Cmd::StyleHeading3),
            C(Cmd::StyleTitle),
            Sep,
            C(Cmd::StyleBrowser),
        ],
    },
    Menu {
        title: "Insert",
        mnemonic: 'I',
        items: &[
            C(Cmd::PageBreak),
            C(Cmd::LineBreak),
            C(Cmd::InsertTab),
            C(Cmd::Date),
            C(Cmd::Bookmark),
            Sep,
            C(Cmd::ListBullet),
            C(Cmd::ListNumber),
            C(Cmd::ListFormat),
            C(Cmd::ListIndent),
            C(Cmd::ListOutdent),
            C(Cmd::ListRestart),
            C(Cmd::ListContinue),
            C(Cmd::ListRemove),
        ],
    },
    Menu {
        title: "Table",
        mnemonic: 'T',
        items: &[
            C(Cmd::TableInsert),
            Sep,
            C(Cmd::TableNextCell),
            C(Cmd::TablePrevCell),
            Sep,
            C(Cmd::TableInsertRowBelow),
            C(Cmd::TableInsertRowAbove),
            C(Cmd::TableInsertColRight),
            C(Cmd::TableInsertColLeft),
            C(Cmd::TableDeleteRow),
            C(Cmd::TableDeleteCol),
            Sep,
            C(Cmd::TableColWidth),
            C(Cmd::TableHeaderRow),
            Sep,
            C(Cmd::TableToText),
            C(Cmd::TableDelete),
        ],
    },
    Menu {
        title: "View",
        mnemonic: 'V',
        items: &[
            C(Cmd::RevealCodes),
            C(Cmd::RevealAllCodes),
            C(Cmd::ToggleView),
            Sep,
            C(Cmd::MenuBar),
            C(Cmd::FkeyLegend),
            Sep,
            C(Cmd::ThemeDefault),
            C(Cmd::ThemeClassic),
            Sep,
            C(Cmd::WordCount),
            C(Cmd::Redraw),
        ],
    },
    Menu {
        title: "Help",
        mnemonic: 'H',
        items: &[
            C(Cmd::Help),
            C(Cmd::Palette),
            Sep,
            C(Cmd::KeyboardModern),
            C(Cmd::KeyboardClassic),
            Sep,
            C(Cmd::About),
        ],
    },
];

/// Width of each title on the bar, including its two-space padding.
pub fn title_width(m: &Menu) -> u16 {
    m.title.len() as u16 + 2
}

/// The bar's x range for menu `i`: (start, end) in screen columns. The bar
/// begins with one space of margin.
pub fn title_span(i: usize) -> (u16, u16) {
    let mut x = 1u16;
    for (k, m) in MENUS.iter().enumerate() {
        let w = title_width(m);
        if k == i {
            return (x, x + w);
        }
        x += w;
    }
    (x, x)
}

/// The menu whose title sits under column `x`.
pub fn menu_at(x: u16) -> Option<usize> {
    (0..MENUS.len()).find(|&i| {
        let (a, b) = title_span(i);
        x >= a && x < b
    })
}

/// The first selectable item of a menu.
pub fn first_item(menu: usize) -> usize {
    MENUS[menu].items.iter().position(|it| matches!(it, Item::Cmd(_))).unwrap_or(0)
}

/// The selectable item after `item` (wrapping), skipping separators.
pub fn next_item(menu: usize, item: usize) -> usize {
    let items = MENUS[menu].items;
    let n = items.len();
    let mut i = item;
    for _ in 0..n {
        i = (i + 1) % n;
        if matches!(items[i], Item::Cmd(_)) {
            return i;
        }
    }
    item
}

/// The selectable item before `item` (wrapping), skipping separators.
pub fn prev_item(menu: usize, item: usize) -> usize {
    let items = MENUS[menu].items;
    let n = items.len();
    let mut i = item;
    for _ in 0..n {
        i = (i + n - 1) % n;
        if matches!(items[i], Item::Cmd(_)) {
            return i;
        }
    }
    item
}

/// The next item (after `from`, wrapping) whose title starts with `c`.
pub fn item_by_letter(menu: usize, from: usize, c: char) -> Option<usize> {
    let items = MENUS[menu].items;
    let n = items.len();
    let c = c.to_ascii_uppercase();
    (1..=n).map(|k| (from + k) % n).find(|&i| match items[i] {
        Item::Cmd(cmd) => crate::commands::info(cmd).title.chars().next().map_or(false, |t| t.to_ascii_uppercase() == c),
        Item::Sep => false,
    })
}

/// The menu whose mnemonic is `c`.
pub fn menu_by_letter(c: char) -> Option<usize> {
    let c = c.to_ascii_uppercase();
    MENUS.iter().position(|m| m.mnemonic == c)
}
