//! Message types for the Elm Architecture

/// All possible user actions and system events
#[derive(Debug, Clone)]
pub enum Message {
    // === Navigation ===
    /// Select a review from the list
    SelectReview(String),
    /// Go back to previous screen
    Back,

    // === List Navigation ===
    /// Move selection up in list
    ListUp,
    /// Move selection down in list
    ListDown,
    /// Page up in list
    ListPageUp,
    /// Page down in list
    ListPageDown,
    /// Go to first item
    ListTop,
    /// Go to last item
    ListBottom,

    // === File Sidebar ===
    /// Move down in sidebar tree
    NextFile,
    /// Move up in sidebar tree
    PrevFile,
    /// Jump to first sidebar item
    SidebarTop,
    /// Jump to last sidebar item
    SidebarBottom,
    /// Select file by index
    SelectFile(usize),
    /// Select sidebar item by row index (mouse click)
    ClickSidebarItem(usize),
    /// Activate current sidebar item (Enter)
    SidebarSelect,

    // === Diff/Content Pane ===
    /// Move cursor up one row
    CursorUp,
    /// Move cursor down one row
    CursorDown,
    /// Move cursor to first row
    CursorTop,
    /// Move cursor to last row
    CursorBottom,
    /// Toggle visual line selection mode (Shift+V)
    VisualToggle,
    /// Scroll content up
    ScrollUp,
    /// Scroll content down
    ScrollDown,
    /// Scroll to top
    ScrollTop,
    /// Scroll to bottom
    ScrollBottom,
    /// Scroll up by half a page
    ScrollHalfPageUp,
    /// Scroll down by half a page
    ScrollHalfPageDown,
    /// Scroll up by 10 lines
    ScrollTenUp,
    /// Scroll down by 10 lines
    ScrollTenDown,
    /// Page up in content
    PageUp,
    /// Page down in content
    PageDown,
    /// Jump to next thread
    NextThread,
    /// Jump to previous thread
    PrevThread,
    /// Expand a thread to show comments
    ExpandThread(String),
    /// Collapse expanded thread
    CollapseThread,

    // === Focus ===
    /// Toggle focus between panes
    ToggleFocus,

    // === Actions ===
    /// Resolve a thread
    ResolveThread(String),
    /// Reopen a resolved thread
    ReopenThread(String),

    // === Filter/View ===
    /// Cycle review list status filter (All → Open → Closed → All)
    CycleStatusFilter,
    /// Activate search input on review list
    SearchActivate,
    /// Append character to search input
    SearchInput(String),
    /// Delete last character from search input
    SearchBackspace,
    /// Delete last word from search input
    SearchDeleteWord,
    /// Clear search input text (stay in search mode)
    SearchClearLine,
    /// Clear and deactivate search
    SearchClear,
    /// Toggle between unified and side-by-side diff view
    ToggleDiffView,
    /// Toggle file sidebar visibility
    ToggleSidebar,
    /// Toggle diff line wrapping
    ToggleDiffWrap,
    /// Open current file in editor
    OpenFileInEditor,

    // === Command Palette ===
    ShowCommandPalette,
    HideCommandPalette,
    CommandPaletteNext,
    CommandPalettePrev,
    CommandPaletteUpdateInput(String),
    CommandPaletteInputBackspace,
    CommandPaletteDeleteWord,
    CommandPaletteExecute,

    // === Commenting ===
    /// Open inline multi-line comment editor (a)
    StartComment,
    /// Open $EDITOR for comment (Shift+A)
    StartCommentExternal,
    EnterCommentMode,
    CommentInput(String),
    CommentInputBackspace,
    CommentNewline,
    CommentCursorUp,
    CommentCursorDown,
    CommentCursorLeft,
    CommentCursorRight,
    CommentHome,
    CommentEnd,
    CommentWordLeft,
    CommentWordRight,
    CommentDeleteWord,
    CommentClearLine,
    SaveComment,
    CancelComment,

    // === Theme Selection ===
    ShowThemePicker,
    ApplyTheme(String),

    // === System ===
    /// Terminal resize event
    Resize {
        width: u16,
        height: u16,
    },
    /// Periodic tick for animations/refresh
    Tick,
    /// Request to quit
    Quit,
    /// No-op (ignore event)
    Noop,
}
