# CodeStory User Guide

Welcome to **CodeStory**, a modern, Rust-based source code explorer inspired by Sourcetrail. CodeStory helps you navigate and understand large or unfamiliar codebases through interactive graphs, snippets, and powerful search.

## Key Features

### 1. Interactive Graph Visualization
- **Focus**: The graph centers on the currently selected symbol.
- **Dependencies**: Shows incoming and outgoing relationships (calls, inheritance, usage).
- **Navigation**: Click any node to select it and update all other views.
- **Tooltips**: Hover over nodes or edges to see their type and signature.

### 2. Code View with Snippets
- **Context-Aware**: Displays the most relevant snippets for your selection.
- **Multiple Modes**: Toggle between **Full File** view and **Snippets** view.
- **Active Highlighting**: The current symbol's locations are highlighted in the code.
- **Navigation**: Click on any highlighted token in the code to navigate to its definition.
- **Search (Ctrl+F)**: Search for text within the current file with real-time highlighting.

### 3. Navigation & History
- **Tabs**: Open multiple symbols in parallel tabs.
- **Back/Forward**: Navigate through your exploration history like a web browser.
- **Keyboard Shortcuts**:
  - `Alt + Left/Right`: Back / Forward
  - `Ctrl + T`: New Tab
  - `Ctrl + W`: Close Tab
  - `Ctrl + 1-9`: Switch to Tab
  - `Ctrl + F`: Search in current file

### 4. Project Management
- **Welcome Screen**: Easily open project folders or access **Recent Projects**.
- **Indexing**: High-performance, parallel indexing powered by tree-sitter.
- **Persistence**: Your UI state (tabs, history) and settings are automatically saved.

## Getting Started

1. **Launch CodeStory**: Start the application.
2. **Open a Project**: On the welcome screen, click "Open Project Folder" and select the root directory of your source code.
3. **Index**: Click the "Index Workspace" button in the status bar.
4. **Explore**: Use the search bar at the top or click nodes in the graph to start navigating your code.

## Configuration

Access **Preferences** via the **File** menu to customize:
- **Theme**: Dark or Light mode.
- **UI Scale**: Adjust for different screen resolutions.
- **Font Size**: Change text size for better readability.
- **Tooltips**: Toggle context-sensitive information.

## IDE Integration

CodeStory supports communication with IDE plugins (compatible with Sourcetrail plugins).
- **Default Port**: 6667 (TCP)
- **Features**: Sync selection between your IDE and CodeStory.

---
*CodeStory is a migration project based on the original Sourcetrail source code.*
