<script lang="ts">
    import { onMount, tick } from "svelte";
    import TopToolbar from "./components/TopToolbar.svelte";
    import Navigator from "./components/Navigator.svelte";
    import SqlEditor from "./components/SqlEditor.svelte";
    import OutputPanel from "./components/OutputPanel.svelte";
    import StatusBar from "./components/StatusBar.svelte";
    import ConnectionDialog from "./components/ConnectionDialog.svelte";
    import ImportDialog from "./components/ImportDialog.svelte";
    import {
        tabs,
        activeTabId,
        activeConnections,
        queryHistoryStore,
        selectedConnId,
        hydrateCachedSchemas,
        refreshMissingConnectionSchemas,
        setActiveConnectionsOrdered,
    } from "./stores/appStore";
    import {
        ListSavedConnections,
        GetQueryHistory,
    } from "../tauri/gen/main/App";
    import { get } from "svelte/store";

    // Tab context menu state
    let tabContextMenu: { tabId: string; x: number; y: number } | null = null;

    // Inline tab editing state
    let editingTabId: string | null = null;
    let editingTitle: string = "";

    // Drag-drop state
    let draggedTabIndex: number | null = null;
    // Index where the tab will be inserted if dropped (0..n)
    let dropTargetIndex: number | null = null;
    // Reference to the tab-bar element for measuring positions
    let tabBarEl: HTMLDivElement;
    // X coordinate for indicator inside tab-bar (px)
    let indicatorX = 0;

    // Tab overflow scroll controls
    let tabsOverflowing = false;
    let canScrollTabsLeft = false;
    let canScrollTabsRight = false;

    // Tab context menu handlers
    function openTabContextMenu(e: MouseEvent, tabId: string) {
        e.preventDefault();
        e.stopPropagation();
        tabContextMenu = { tabId, x: e.clientX, y: e.clientY };
    }

    function closeTabContextMenu() {
        tabContextMenu = null;
    }

    function handleTabAction(
        action:
            | "rename"
            | "duplicate"
            | "closeOthers"
            | "closeRight"
            | "closeLeft",
    ) {
        if (!tabContextMenu) return;
        const { tabId } = tabContextMenu;
        tabContextMenu = null;

        switch (action) {
            case "rename":
                const tab = get(tabs).find((t) => t.id === tabId);
                if (tab) {
                    editingTabId = tabId;
                    editingTitle = tab.title;
                    // Focus the input after render
                    setTimeout(() => {
                        const input = document.querySelector(
                            `.tab[data-tab-id="${tabId}"] input`,
                        ) as HTMLInputElement;
                        if (input) {
                            input.focus();
                            input.select();
                        }
                    }, 0);
                }
                break;
            case "duplicate":
                tabs.duplicateTab(tabId);
                break;
            case "closeOthers":
                tabs.closeOtherTabs(tabId);
                break;
            case "closeRight":
                tabs.closeTabsToRight(tabId);
                break;
            case "closeLeft":
                tabs.closeTabsToLeft(tabId);
                break;
        }
    }

    // Drag-drop handlers
    function onTabDragStart(e: DragEvent, index: number) {
        draggedTabIndex = index;
        dropTargetIndex = null;
        e.dataTransfer!.effectAllowed = "move";
        e.dataTransfer!.setData("text/plain", String(index));
    }

    // Shared helper: given mouse clientX, compute insertion index and indicator position
    function updateDropTarget(clientX: number) {
        if (!tabBarEl) return;
        const barRect = tabBarEl.getBoundingClientRect();
        const children = Array.from(
            tabBarEl.querySelectorAll<HTMLElement>(".tab"),
        );

        // Default: append after all tabs
        let insertAt = children.length;
        let newIndicatorX =
            children.length > 0
                ? children[children.length - 1].getBoundingClientRect().right -
                  barRect.left
                : 0;

        for (let i = 0; i < children.length; i++) {
            const rect = children[i].getBoundingClientRect();
            const mid = rect.left + rect.width / 2;
            if (clientX < mid) {
                insertAt = i;
                newIndicatorX = rect.left - barRect.left;
                break;
            }
        }

        dropTargetIndex = insertAt;
        indicatorX = newIndicatorX;
    }

    function onTabBarDragOver(e: DragEvent) {
        e.preventDefault();
        e.dataTransfer!.dropEffect = "move";
        updateDropTarget(e.clientX);
    }

    function onTabBarDrop(e: DragEvent) {
        e.preventDefault();
        const from = draggedTabIndex;
        const to = dropTargetIndex;
        if (from !== null && to !== null && from !== to) {
            // After splice(from, 1), indices > from shift left by 1
            const adjustedTo = from < to ? to - 1 : to;
            tabs.reorderTabs(from, adjustedTo);
        }
        draggedTabIndex = null;
        dropTargetIndex = null;
        indicatorX = 0;
    }

    function onTabDragEnd() {
        draggedTabIndex = null;
        dropTargetIndex = null;
        indicatorX = 0;
    }

    function updateTabOverflowState() {
        if (!tabBarEl) return;
        const maxScrollLeft = Math.max(0, tabBarEl.scrollWidth - tabBarEl.clientWidth);
        tabsOverflowing = maxScrollLeft > 1;
        canScrollTabsLeft = tabBarEl.scrollLeft > 1;
        canScrollTabsRight = tabBarEl.scrollLeft < maxScrollLeft - 1;
    }

    function onTabBarScroll() {
        updateTabOverflowState();
    }

    function scrollTabs(direction: -1 | 1) {
        if (!tabBarEl) return;
        const step = Math.max(180, Math.floor(tabBarEl.clientWidth * 0.65));
        tabBarEl.scrollBy({ left: direction * step, behavior: "smooth" });
        // Keep button state responsive even while smooth scrolling is ongoing.
        requestAnimationFrame(updateTabOverflowState);
    }

    function saveTabTitle() {
        if (editingTabId && editingTitle.trim()) {
            tabs.renameTab(editingTabId, editingTitle.trim());
        }
        editingTabId = null;
        editingTitle = "";
    }

    function cancelTabEdit() {
        editingTabId = null;
        editingTitle = "";
    }

    function getConnectionConfig(connId: string) {
        return $activeConnections.find((c) => c.config.id === connId)?.config;
    }

    function hasCustomTabColor(connId: string) {
        const cfg = getConnectionConfig(connId);
        return !!cfg?.tabColor;
    }

    function getTabCustomStyle(connId: string) {
        const cfg = getConnectionConfig(connId);
        if (!cfg?.tabColor) return "";
        const textColor = cfg.tabTextBlack ? "#000000" : "var(--text)";
        return `--tab-custom-bg: ${cfg.tabColor}; --tab-custom-text: ${textColor};`;
    }

    // Pane sizes
    let navWidth = 240;
    let editorRatio = 0.55; // fraction of main area for SQL editor
    let draggingNav = false;
    let draggingPane = false;
    let mainHeight = 0;

    function startNavDrag(e: MouseEvent) {
        draggingNav = true;
        e.preventDefault();
    }
    function startPaneDrag(e: MouseEvent) {
        draggingPane = true;
        e.preventDefault();
    }

    function onMouseMove(e: MouseEvent) {
        if (draggingNav) {
            navWidth = Math.max(160, Math.min(500, e.clientX));
        }
        if (draggingPane && mainHeight > 0) {
            const mainEl = document.getElementById("main-area");
            if (mainEl) {
                const rect = mainEl.getBoundingClientRect();
                const toolbarH = 32; // approx editor toolbar height
                const ratio =
                    (e.clientY - rect.top - toolbarH) /
                    (rect.height - toolbarH);
                editorRatio = Math.max(0.15, Math.min(0.85, ratio));
            }
        }
    }

    function onMouseUp() {
        draggingNav = false;
        draggingPane = false;
    }

    onMount(() => {
        let mounted = true;
        const onResize = () => updateTabOverflowState();
        window.addEventListener("resize", onResize);

        // Initialize active tab
        const $tabs = get(tabs);
        if ($tabs.length > 0) activeTabId.set($tabs[0].id);

        void (async () => {
            // Load saved connections
            try {
                const saved = await ListSavedConnections();
                if (mounted && saved && saved.length > 0) {
                    setActiveConnectionsOrdered(
                        saved.map((cfg) => ({
                            config: cfg,
                            schema: null,
                            schemaLoading: false,
                            schemaError: null,
                        })),
                    );
                    selectedConnId.set(saved[0].id);
                    // Hydrate cached schemas
                    await hydrateCachedSchemas();
                    // Fresh installs or newly-added saved connections collect schema in the background.
                    await refreshMissingConnectionSchemas();
                }
            } catch (_) {}

            // Load query history
            try {
                const hist = await GetQueryHistory(200);
                if (mounted && hist) queryHistoryStore.set(hist);
            } catch (_) {}

            await tick();
            if (mounted) updateTabOverflowState();
        })();

        return () => {
            mounted = false;
            window.removeEventListener("resize", onResize);
        };
    });

    $: {
        // Recalculate whenever tab list or title-edit mode changes widths.
        $tabs;
        editingTabId;
        tick().then(updateTabOverflowState);
    }
</script>

<svelte:window
    on:mousemove={onMouseMove}
    on:mouseup={onMouseUp}
    on:click={closeTabContextMenu}
/>

<div class="app">
    <TopToolbar />

    <div class="workspace" bind:clientHeight={mainHeight}>
        <!-- Navigator sidebar -->
        <div
            class="nav-pane"
            style="width:{navWidth}px; min-width:{navWidth}px;"
        >
            <Navigator />
        </div>

        <!-- Drag handle for nav -->
        <button
            class="drag-handle-v"
            on:mousedown={startNavDrag}
            type="button"
            aria-label="Resize navigator"
        ></button>

        <!-- Main content area -->
        <div class="main-area" id="main-area">
            <!-- Tab bar -->
            <div class="tab-bar">
                <div
                    class="tab-scroll"
                    role="tablist"
                    tabindex="0"
                    bind:this={tabBarEl}
                    on:scroll={onTabBarScroll}
                    on:dragover={onTabBarDragOver}
                    on:drop={onTabBarDrop}
                >
                    <!-- insertion indicator -->
                    <div
                        class="insertion-indicator"
                        style="left:{indicatorX}px; display:{dropTargetIndex !==
                        null
                            ? 'block'
                            : 'none'}"
                        aria-hidden="true"
                    ></div>

                    {#each $tabs as tab, i (tab.id)}
                        <button
                            class="tab"
                            class:active={$activeTabId === tab.id}
                            class:dragging={draggedTabIndex === i}
                            class:has-custom-color={hasCustomTabColor(tab.connId)}
                            data-tab-id={tab.id}
                            style={getTabCustomStyle(tab.connId)}
                            on:click={() => {
                                if (editingTabId !== tab.id)
                                    activeTabId.set(tab.id);
                            }}
                            on:contextmenu={(e) => openTabContextMenu(e, tab.id)}
                            draggable="true"
                            on:dragstart={(e) => onTabDragStart(e, i)}
                            on:dragend={onTabDragEnd}
                            role="tab"
                            aria-selected={$activeTabId === tab.id}
                        >
                            {#if editingTabId === tab.id}
                                <input
                                    class="tab-title-input"
                                    bind:value={editingTitle}
                                    on:keydown={(e) => {
                                        if (e.key === "Enter") {
                                            saveTabTitle();
                                        } else if (e.key === "Escape") {
                                            cancelTabEdit();
                                        }
                                    }}
                                    on:blur={saveTabTitle}
                                    on:click|stopPropagation
                                />
                            {:else}
                                <span class="tab-title">{tab.title}</span>
                            {/if}
                            {#if tab.running}
                                <span class="tab-spinner">⟳</span>
                            {/if}
                            <span
                                class="tab-close"
                                on:click|stopPropagation={() => tabs.remove(tab.id)}
                                role="button"
                                tabindex="0"
                                on:keydown={(e) =>
                                    e.key === "Enter" && tabs.remove(tab.id)}
                                aria-label="Close tab">✕</span
                            >
                        </button>
                    {/each}
                </div>

                <div class="tab-controls">
                    {#if tabsOverflowing}
                        <button
                            class="tab-scroll-btn"
                            on:click={() => scrollTabs(-1)}
                            disabled={!canScrollTabsLeft}
                            aria-label="Scroll tabs left"
                            title="Scroll tabs left"
                        >
                            ◀
                        </button>
                        <button
                            class="tab-scroll-btn"
                            on:click={() => scrollTabs(1)}
                            disabled={!canScrollTabsRight}
                            aria-label="Scroll tabs right"
                            title="Scroll tabs right"
                        >
                            ▶
                        </button>
                    {/if}

                    <button
                        class="tab-add"
                        on:click={() => {
                            tabs.add(get(selectedConnId));
                            const t = get(tabs);
                            activeTabId.set(t[t.length - 1].id);
                        }}
                        title="New query tab"
                        aria-label="Add tab">+</button
                    >
                </div>
            </div>

            <!-- Editor + Output split -->
            <div class="editor-output-split">
                <div
                    class="editor-pane"
                    style="flex: {editorRatio} 0 0; min-height: 80px;"
                >
                    {#each $tabs as tab (tab.id)}
                        <div
                            class="tab-panel"
                            class:active={$activeTabId === tab.id}
                        >
                            <SqlEditor tabId={tab.id} />
                        </div>
                    {/each}
                </div>

                <!-- Horizontal drag handle -->
                <button
                    class="drag-handle-h"
                    on:mousedown={startPaneDrag}
                    type="button"
                    aria-label="Resize output panel"
                ></button>

                <div
                    class="output-pane"
                    style="flex: {1 - editorRatio} 0 0; min-height: 60px;"
                >
                    <OutputPanel />
                </div>
            </div>
        </div>
    </div>

    <StatusBar />
</div>

<ConnectionDialog />
<ImportDialog />

{#if tabContextMenu}
    <div
        class="tab-context-menu"
        style="left:{tabContextMenu.x}px; top:{tabContextMenu.y}px"
        role="menu"
    >
        <button role="menuitem" on:click={() => handleTabAction("rename")}>
            Rename Tab
        </button>
        <button role="menuitem" on:click={() => handleTabAction("duplicate")}>
            Duplicate Tab
        </button>
        <div class="context-separator"></div>
        <button role="menuitem" on:click={() => handleTabAction("closeOthers")}>
            Close Other Tabs
        </button>
        <button role="menuitem" on:click={() => handleTabAction("closeRight")}>
            Close Tabs to the Right
        </button>
        <button role="menuitem" on:click={() => handleTabAction("closeLeft")}>
            Close Tabs to the Left
        </button>
    </div>
{/if}

<style>
    :global(*) {
        box-sizing: border-box;
    }
    :global(body) {
        margin: 0;
        padding: 0;
        overflow: hidden;
        background: var(--bg);
        color: var(--text);
        font-family:
            -apple-system, BlinkMacSystemFont, "Segoe UI", Roboto, sans-serif;
        font-size: calc(13px * var(--app-font-scale));
    }
    :global(:root) {
        --bg: #12121a;
        --bg-panel: #1a1a24;
        --bg-surface: #1e1e2e;
        --bg-toolbar: #16161f;
        --bg-editor: #0f0f17;
        --bg-input: #1e1e2e;
        --bg-hover: rgba(255, 255, 255, 0.04);
        --bg-selected: rgba(99, 102, 241, 0.15);
        --bg-badge: #252536;
        --bg-row-alt: rgba(255, 255, 255, 0.02);
        --border: #2e2e40;
        --border-subtle: #1e1e2d;
        --text: #e2e2f0;
        --text-muted: #888898;
        --text-dim: #aaaabc;
        --accent: #6366f1;
        --accent-hover: #818cf8;
        --success: #34d399;
        --error: #f87171;
    }

    .app {
        display: flex;
        flex-direction: column;
        height: 100vh;
        overflow: hidden;
    }
    .workspace {
        display: flex;
        flex: 1;
        overflow: hidden;
        min-height: 0;
    }
    .nav-pane {
        flex-shrink: 0;
        overflow: hidden;
        display: flex;
        flex-direction: column;
    }
    .main-area {
        flex: 1;
        display: flex;
        flex-direction: column;
        overflow: hidden;
        min-width: 0;
    }

    .drag-handle-v {
        width: 4px;
        cursor: col-resize;
        flex-shrink: 0;
        background: var(--border);
        transition: background 0.15s;
        border: none;
        padding: 0;
        appearance: none;
        display: block;
    }
    .drag-handle-v:hover {
        background: var(--accent);
    }
    .drag-handle-h {
        height: 4px;
        cursor: row-resize;
        flex-shrink: 0;
        background: var(--border);
        transition: background 0.15s;
        border: none;
        padding: 0;
        appearance: none;
        display: block;
    }
    .drag-handle-h:hover {
        background: var(--accent);
    }

    .tab-bar {
        display: flex;
        align-items: center;
        background: var(--bg-toolbar);
        border-bottom: 1px solid var(--border);
        overflow: hidden;
        flex-shrink: 0;
        min-width: 0;
    }
    .tab-scroll {
        flex: 1;
        min-width: 0;
        display: flex;
        align-items: center;
        overflow-x: auto;
        overflow-y: hidden;
        scrollbar-width: none;
        position: relative; /* needed for insertion indicator positioning */
    }
    .tab-scroll::-webkit-scrollbar {
        display: none;
    }

    /* Insertion indicator shown while dragging a tab */
    .insertion-indicator {
        position: absolute;
        top: 8px;
        bottom: 8px;
        width: 6px;
        background: #ffffff;
        border-radius: 2px;
        z-index: 50;
        box-shadow: 0 0 0 2px rgba(255, 255, 255, 0.05);
        pointer-events: none;
        transition: left 0.08s linear;
    }
    .tab {
        display: flex;
        align-items: center;
        gap: 6px;
        padding: 6px 14px;
        background: none;
        border: none;
        border-bottom: 2px solid transparent;
        color: var(--text-muted);
        font-size: calc(12px * var(--app-font-scale));
        cursor: pointer;
        white-space: nowrap;
        min-width: 80px;
    }
    .tab:hover {
        color: var(--text);
        background: var(--bg-hover);
    }
    .tab.active {
        color: var(--text);
        border-bottom-color: var(--accent);
        background: var(--bg-surface);
    }
    .tab.has-custom-color {
        background: var(--tab-custom-bg);
        color: var(--tab-custom-text);
    }
    .tab.has-custom-color:hover {
        background: var(--tab-custom-bg);
        color: var(--tab-custom-text);
        filter: brightness(1.03);
    }
    .tab.has-custom-color.active {
        background: var(--tab-custom-bg);
        color: var(--tab-custom-text);
        border-bottom-color: var(--tab-custom-text);
    }
    .tab-title {
        flex: 1;
    }
    .tab-title-input {
        flex: 1;
        background: transparent;
        border: 1px solid var(--accent);
        border-radius: 2px;
        color: var(--text);
        font-size: calc(12px * var(--app-font-scale));
        padding: 2px 4px;
        outline: none;
        min-width: 0;
    }
    .tab-close {
        color: var(--text-muted);
        font-size: calc(11px * var(--app-font-scale));
        opacity: 0;
        padding: 0 2px;
        border-radius: 2px;
    }
    .tab.has-custom-color .tab-close {
        color: inherit;
    }
    .tab:hover .tab-close {
        opacity: 0.7;
    }
    .tab-close:hover {
        opacity: 1 !important;
        color: var(--text);
        background: var(--bg-hover);
    }
    .tab.has-custom-color .tab-close:hover {
        color: inherit;
        background: rgba(0, 0, 0, 0.12);
    }
    .tab-spinner {
        animation: spin 1s linear infinite;
        display: inline-block;
    }
    @keyframes spin {
        from {
            transform: rotate(0deg);
        }
        to {
            transform: rotate(360deg);
        }
    }
    .tab-add {
        padding: 6px 12px;
        background: none;
        border: none;
        color: var(--text-muted);
        cursor: pointer;
        font-size: calc(14px * var(--app-font-scale));
        flex-shrink: 0;
    }
    .tab-add:hover {
        color: var(--text);
    }
    .tab-controls {
        display: flex;
        align-items: center;
        flex-shrink: 0;
        border-left: 1px solid var(--border-subtle);
        background: var(--bg-toolbar);
    }
    .tab-scroll-btn {
        width: 26px;
        height: 26px;
        margin-left: 4px;
        border: 1px solid var(--border);
        border-radius: 4px;
        background: var(--bg-surface);
        color: var(--text);
        cursor: pointer;
        line-height: 1;
        font-size: calc(11px * var(--app-font-scale));
        flex-shrink: 0;
    }
    .tab-scroll-btn:hover:not(:disabled) {
        border-color: var(--accent);
        color: var(--accent-hover);
    }
    .tab-scroll-btn:disabled {
        opacity: 0.45;
        cursor: default;
    }

    .editor-output-split {
        display: flex;
        flex-direction: column;
        flex: 1;
        overflow: hidden;
        min-height: 0;
    }
    .editor-pane {
        overflow: hidden;
        min-height: 0;
    }
    .output-pane {
        overflow: hidden;
        min-height: 0;
    }

    .tab-panel {
        display: none;
        height: 100%;
    }
    .tab-panel.active {
        display: flex;
        flex-direction: column;
        height: 100%;
    }

    .tab-context-menu {
        position: fixed;
        z-index: 300;
        background: var(--bg-surface);
        border: 1px solid var(--border);
        border-radius: 4px;
        min-width: 160px;
        box-shadow: 0 4px 16px rgba(0, 0, 0, 0.4);
        overflow: hidden;
    }
    .tab-context-menu button {
        display: block;
        width: 100%;
        text-align: left;
        padding: 8px 16px;
        background: none;
        border: none;
        color: var(--text);
        font-size: calc(13px * var(--app-font-scale));
        cursor: pointer;
    }
    .tab-context-menu button:hover {
        background: var(--bg-hover);
    }
    .context-separator {
        height: 1px;
        background: var(--border);
        margin: 3px 0;
    }

    .tab.dragging {
        opacity: 0.5;
    }
</style>
