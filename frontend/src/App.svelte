<script lang="ts">
  import { onMount } from 'svelte';
  import TopToolbar from './components/TopToolbar.svelte';
  import Navigator from './components/Navigator.svelte';
  import SqlEditor from './components/SqlEditor.svelte';
  import OutputPanel from './components/OutputPanel.svelte';
  import StatusBar from './components/StatusBar.svelte';
  import ConnectionDialog from './components/ConnectionDialog.svelte';
  import { tabs, activeTabId, activeConnections, queryHistoryStore, selectedConnId, hydrateCachedSchemas } from './stores/appStore';
  import { ListSavedConnections, GetQueryHistory } from '../wailsjs/go/main/App';
  import { get } from 'svelte/store';

  // Tab context menu state
  let tabContextMenu: { tabId: string; x: number; y: number } | null = null;

  // Drag-drop state
  let draggedTabIndex: number | null = null;

  // Tab context menu handlers
  function openTabContextMenu(e: MouseEvent, tabId: string) {
    e.preventDefault();
    e.stopPropagation();
    tabContextMenu = { tabId, x: e.clientX, y: e.clientY };
  }

  function closeTabContextMenu() {
    tabContextMenu = null;
  }

  function handleTabAction(action: 'rename' | 'duplicate' | 'closeOthers' | 'closeRight' | 'closeLeft') {
    if (!tabContextMenu) return;
    const { tabId } = tabContextMenu;
    tabContextMenu = null;

    switch (action) {
      case 'rename':
        const newTitle = window.prompt('Enter new tab name:', get(tabs).find(t => t.id === tabId)?.title || 'Query');
        if (newTitle && newTitle.trim()) {
          tabs.renameTab(tabId, newTitle.trim());
        }
        break;
      case 'duplicate':
        tabs.duplicateTab(tabId);
        break;
      case 'closeOthers':
        tabs.closeOtherTabs(tabId);
        break;
      case 'closeRight':
        tabs.closeTabsToRight(tabId);
        break;
      case 'closeLeft':
        tabs.closeTabsToLeft(tabId);
        break;
    }
  }

  // Drag-drop handlers
  function onTabDragStart(e: DragEvent, index: number) {
    draggedTabIndex = index;
    e.dataTransfer!.effectAllowed = 'move';
  }

  function onTabDragOver(e: DragEvent) {
    e.preventDefault();
    e.dataTransfer!.dropEffect = 'move';
  }

  function onTabDrop(e: DragEvent, dropIndex: number) {
    e.preventDefault();
    if (draggedTabIndex !== null && draggedTabIndex !== dropIndex) {
      tabs.reorderTabs(draggedTabIndex, dropIndex);
    }
    draggedTabIndex = null;
  }

  function onTabDragEnd() {
    draggedTabIndex = null;
  }

  // Pane sizes
  let navWidth = 240;
  let editorRatio = 0.55; // fraction of main area for SQL editor
  let draggingNav = false;
  let draggingPane = false;
  let mainHeight = 0;

  function startNavDrag(e: MouseEvent) { draggingNav = true; e.preventDefault(); }
  function startPaneDrag(e: MouseEvent) { draggingPane = true; e.preventDefault(); }

  function onMouseMove(e: MouseEvent) {
    if (draggingNav) {
      navWidth = Math.max(160, Math.min(500, e.clientX));
    }
    if (draggingPane && mainHeight > 0) {
      const mainEl = document.getElementById('main-area');
      if (mainEl) {
        const rect = mainEl.getBoundingClientRect();
        const toolbarH = 32; // approx editor toolbar height
        const ratio = (e.clientY - rect.top - toolbarH) / (rect.height - toolbarH);
        editorRatio = Math.max(0.15, Math.min(0.85, ratio));
      }
    }
  }

  function onMouseUp() { draggingNav = false; draggingPane = false; }

  onMount(async () => {
    // Initialize active tab
    const $tabs = get(tabs);
    if ($tabs.length > 0) activeTabId.set($tabs[0].id);

    // Load saved connections
    try {
      const saved = await ListSavedConnections();
      if (saved && saved.length > 0) {
        activeConnections.set(saved.map(cfg => ({ config: cfg, schema: null, schemaLoading: false, schemaError: null })));
        selectedConnId.set(saved[0].id);
        // Hydrate cached schemas
        await hydrateCachedSchemas();
      }
    } catch (_) {}

    // Load query history
    try {
      const hist = await GetQueryHistory(200);
      if (hist) queryHistoryStore.set(hist);
    } catch (_) {}
  });
</script>

<svelte:window on:mousemove={onMouseMove} on:mouseup={onMouseUp} on:click={closeTabContextMenu} />

<div class="app">
  <TopToolbar />

  <div class="workspace" bind:clientHeight={mainHeight}>
    <!-- Navigator sidebar -->
    <div class="nav-pane" style="width:{navWidth}px; min-width:{navWidth}px;">
      <Navigator />
    </div>

    <!-- Drag handle for nav -->
    <div class="drag-handle-v" on:mousedown={startNavDrag} role="separator" aria-orientation="vertical" aria-label="Resize navigator"></div>

    <!-- Main content area -->
    <div class="main-area" id="main-area">
      <!-- Tab bar -->
      <div class="tab-bar" role="tablist">
        {#each $tabs as tab, i (tab.id)}
          <button
            class="tab"
            class:active={$activeTabId === tab.id}
            class:dragging={draggedTabIndex === i}
            on:click={() => activeTabId.set(tab.id)}
            on:contextmenu={(e) => openTabContextMenu(e, tab.id)}
            draggable="true"
            on:dragstart={(e) => onTabDragStart(e, i)}
            on:dragover={onTabDragOver}
            on:drop={(e) => onTabDrop(e, i)}
            on:dragend={onTabDragEnd}
            role="tab"
            aria-selected={$activeTabId === tab.id}
          >
            <span class="tab-title">{tab.title}</span>
            {#if tab.running}
              <span class="tab-spinner">⟳</span>
            {/if}
            <span
              class="tab-close"
              on:click|stopPropagation={() => tabs.remove(tab.id)}
              role="button"
              tabindex="0"
              on:keydown={e => e.key === 'Enter' && tabs.remove(tab.id)}
              aria-label="Close tab"
            >✕</span>
          </button>
        {/each}
        <button class="tab-add" on:click={() => { tabs.add(get(selectedConnId)); const t = get(tabs); activeTabId.set(t[t.length-1].id); }} title="New query tab" aria-label="Add tab">+</button>
      </div>

      <!-- Editor + Output split -->
      <div class="editor-output-split">
        <div class="editor-pane" style="flex: {editorRatio} 0 0; min-height: 80px;">
          {#each $tabs as tab (tab.id)}
            <div class="tab-panel" class:active={$activeTabId === tab.id}>
              <SqlEditor tabId={tab.id} />
            </div>
          {/each}
        </div>

        <!-- Horizontal drag handle -->
        <div class="drag-handle-h" on:mousedown={startPaneDrag} role="separator" aria-orientation="horizontal" aria-label="Resize output panel"></div>

        <div class="output-pane" style="flex: {1 - editorRatio} 0 0; min-height: 60px;">
          <OutputPanel />
        </div>
      </div>
    </div>
  </div>

  <StatusBar />
</div>

<ConnectionDialog />

{#if tabContextMenu}
  <div
    class="tab-context-menu"
    style="left:{tabContextMenu.x}px; top:{tabContextMenu.y}px"
    role="menu"
  >
    <button role="menuitem" on:click={() => handleTabAction('rename')}>
      Rename Tab
    </button>
    <button role="menuitem" on:click={() => handleTabAction('duplicate')}>
      Duplicate Tab
    </button>
    <div class="context-separator"></div>
    <button role="menuitem" on:click={() => handleTabAction('closeOthers')}>
      Close Other Tabs
    </button>
    <button role="menuitem" on:click={() => handleTabAction('closeRight')}>
      Close Tabs to the Right
    </button>
    <button role="menuitem" on:click={() => handleTabAction('closeLeft')}>
      Close Tabs to the Left
    </button>
  </div>
{/if}



<style>
  :global(*) { box-sizing: border-box; }
  :global(body) {
    margin: 0; padding: 0; overflow: hidden;
    background: var(--bg); color: var(--text);
    font-family: -apple-system, BlinkMacSystemFont, 'Segoe UI', Roboto, sans-serif;
    font-size: 13px;
  }
  :global(:root) {
    --bg: #12121a;
    --bg-panel: #1a1a24;
    --bg-surface: #1e1e2e;
    --bg-toolbar: #16161f;
    --bg-editor: #0f0f17;
    --bg-input: #1e1e2e;
    --bg-hover: rgba(255,255,255,0.04);
    --bg-selected: rgba(99,102,241,0.15);
    --bg-badge: #252536;
    --bg-row-alt: rgba(255,255,255,0.02);
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
    display: flex; flex-direction: column;
    height: 100vh; overflow: hidden;
  }
  .workspace {
    display: flex; flex: 1; overflow: hidden;
    min-height: 0;
  }
  .nav-pane {
    flex-shrink: 0; overflow: hidden;
    display: flex; flex-direction: column;
  }
  .main-area {
    flex: 1; display: flex; flex-direction: column;
    overflow: hidden; min-width: 0;
  }

  .drag-handle-v {
    width: 4px; cursor: col-resize; flex-shrink: 0;
    background: var(--border);
    transition: background 0.15s;
  }
  .drag-handle-v:hover { background: var(--accent); }
  .drag-handle-h {
    height: 4px; cursor: row-resize; flex-shrink: 0;
    background: var(--border);
    transition: background 0.15s;
  }
  .drag-handle-h:hover { background: var(--accent); }

  .tab-bar {
    display: flex; align-items: center;
    background: var(--bg-toolbar);
    border-bottom: 1px solid var(--border);
    overflow-x: auto; flex-shrink: 0;
    scrollbar-width: none;
  }
  .tab-bar::-webkit-scrollbar { display: none; }
  .tab {
    display: flex; align-items: center; gap: 6px;
    padding: 6px 14px; background: none; border: none;
    border-bottom: 2px solid transparent;
    color: var(--text-muted); font-size: 12px; cursor: pointer;
    white-space: nowrap; min-width: 80px;
  }
  .tab:hover { color: var(--text); background: var(--bg-hover); }
  .tab.active { color: var(--text); border-bottom-color: var(--accent); background: var(--bg-surface); }
  .tab-title { flex: 1; }
  .tab-close {
    color: var(--text-muted); font-size: 11px; opacity: 0;
    padding: 0 2px; border-radius: 2px;
  }
  .tab:hover .tab-close { opacity: 0.7; }
  .tab-close:hover { opacity: 1 !important; color: var(--text); background: var(--bg-hover); }
  .tab-spinner { animation: spin 1s linear infinite; display: inline-block; }
  @keyframes spin { from { transform: rotate(0deg); } to { transform: rotate(360deg); } }
  .tab-add {
    padding: 6px 12px; background: none; border: none;
    color: var(--text-muted); cursor: pointer; font-size: 14px;
    flex-shrink: 0;
  }
  .tab-add:hover { color: var(--text); }

  .editor-output-split {
    display: flex; flex-direction: column;
    flex: 1; overflow: hidden; min-height: 0;
  }
  .editor-pane { overflow: hidden; min-height: 0; }
  .output-pane { overflow: hidden; min-height: 0; }

  .tab-panel { display: none; height: 100%; }
  .tab-panel.active { display: flex; flex-direction: column; height: 100%; }

  .tab-context-menu {
    position: fixed; z-index: 300;
    background: var(--bg-surface); border: 1px solid var(--border);
    border-radius: 4px; min-width: 160px;
    box-shadow: 0 4px 16px rgba(0,0,0,0.4);
    overflow: hidden;
  }
  .tab-context-menu button {
    display: block; width: 100%; text-align: left;
    padding: 8px 16px; background: none; border: none;
    color: var(--text); font-size: 13px; cursor: pointer;
  }
  .tab-context-menu button:hover { background: var(--bg-hover); }
  .context-separator {
    height: 1px; background: var(--border); margin: 3px 0;
  }

  .tab.dragging { opacity: 0.5; }
</style>
