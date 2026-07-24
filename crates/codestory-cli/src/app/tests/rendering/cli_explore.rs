use crate::args::Cli;
use crate::explore::{ExploreTuiAction, ExploreTuiState, explore_tui_action};
use clap::Parser;

#[test]
fn all_existing_commands_accept_output_file() {
    let commands = [
        vec!["codestory-cli", "index", "--output-file", "out.md"],
        vec!["codestory-cli", "ground", "--output-file", "out.md"],
        vec![
            "codestory-cli",
            "search",
            "--query",
            "needle",
            "--output-file",
            "out.md",
        ],
        vec![
            "codestory-cli",
            "symbol",
            "--query",
            "Foo",
            "--output-file",
            "out.md",
        ],
        vec![
            "codestory-cli",
            "trail",
            "--query",
            "Foo",
            "--hide-speculative",
            "--format",
            "dot",
            "--output-file",
            "out.md",
        ],
        vec![
            "codestory-cli",
            "snippet",
            "--query",
            "Foo",
            "--output-file",
            "out.md",
        ],
        vec![
            "codestory-cli",
            "task",
            "brief",
            "--prompt",
            "Implement issue 507",
            "--output-file",
            "out.md",
        ],
        vec![
            "codestory-cli",
            "query",
            "search(query: 'Foo') | limit(1)",
            "--output-file",
            "out.md",
        ],
        vec!["codestory-cli", "doctor", "--output-file", "out.md"],
        vec![
            "codestory-cli",
            "explore",
            "--query",
            "Foo",
            "--no-tui",
            "--output-file",
            "out.md",
        ],
        vec![
            "codestory-cli",
            "bookmark",
            "add",
            "--id",
            "1",
            "--output-file",
            "out.md",
        ],
        vec![
            "codestory-cli",
            "bookmark",
            "list",
            "--output-file",
            "out.md",
        ],
        vec![
            "codestory-cli",
            "bookmark",
            "remove",
            "1",
            "--output-file",
            "out.md",
        ],
    ];

    for command in commands {
        Cli::try_parse_from(command).expect("command should parse --output-file");
    }
}

#[test]
fn explore_tui_keyboard_state_reaches_every_pane() {
    let mut state = ExploreTuiState::new(6);
    for expected in 1..6 {
        assert!(!state.apply(ExploreTuiAction::NextPane));
        assert_eq!(state.selected, expected);
    }
    assert!(!state.apply(ExploreTuiAction::NextPane));
    assert_eq!(state.selected, 0);

    assert!(!state.apply(ExploreTuiAction::PreviousPane));
    assert_eq!(state.selected, 5);
    assert!(!state.apply(ExploreTuiAction::ScrollDown(12)));
    assert_eq!(state.scroll[5], 12);
    assert!(!state.apply(ExploreTuiAction::ScrollUp(5)));
    assert_eq!(state.scroll[5], 7);
    assert!(!state.apply(ExploreTuiAction::Home));
    assert_eq!(state.scroll[5], 0);
    assert!(state.apply(ExploreTuiAction::Quit));
}

#[test]
fn explore_tui_key_mapping_covers_keyboard_only_controls() {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    assert_eq!(
        explore_tui_action(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE)),
        ExploreTuiAction::NextPane
    );
    assert_eq!(
        explore_tui_action(KeyEvent::new(KeyCode::BackTab, KeyModifiers::SHIFT)),
        ExploreTuiAction::PreviousPane
    );
    assert_eq!(
        explore_tui_action(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::NONE)),
        ExploreTuiAction::ScrollDown(1)
    );
    assert_eq!(
        explore_tui_action(KeyEvent::new(KeyCode::PageUp, KeyModifiers::NONE)),
        ExploreTuiAction::ScrollUp(10)
    );
    assert_eq!(
        explore_tui_action(KeyEvent::new(KeyCode::Char('q'), KeyModifiers::NONE)),
        ExploreTuiAction::Quit
    );
    assert_eq!(
        explore_tui_action(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL)),
        ExploreTuiAction::Quit
    );
}
