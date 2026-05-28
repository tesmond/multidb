<script lang="ts">
  let title = '';
  let isOpen = false;
  let resolveCallback: ((title: string | null) => void) | null = null;

  export function open(initialTitle: string): Promise<string | null> {
    return new Promise((resolve) => {
      title = initialTitle ?? '';
      isOpen = true;
      resolveCallback = resolve;
    });
  }

  function handleSave() {
    const trimmed = title.trim();
    if (!trimmed) return;
    resolveCallback?.(trimmed);
    isOpen = false;
  }

  function handleCancel() {
    resolveCallback?.(null);
    isOpen = false;
  }

  function handleKeydown(e: KeyboardEvent) {
    if (e.key === 'Enter') {
      handleSave();
    } else if (e.key === 'Escape') {
      handleCancel();
    }
  }
</script>

{#if isOpen}
  <!-- svelte-ignore a11y-click-events-have-key-events -->
  <div class="dialog-backdrop" on:click={handleCancel} role="presentation"></div>
  <div class="dialog">
    <div class="dialog-header">
      <h2>Edit Query Title</h2>
    </div>
    <div class="dialog-body">
      <label for="edit-title-input">Query Title</label>
      <!-- svelte-ignore a11y-autofocus -->
      <input
        id="edit-title-input"
        type="text"
        bind:value={title}
        on:keydown={handleKeydown}
        autofocus
      />
    </div>
    <div class="dialog-footer">
      <button class="btn-cancel" on:click={handleCancel}>Cancel</button>
      <button class="btn-save" on:click={handleSave} disabled={!title.trim()}>Save</button>
    </div>
  </div>
{/if}

<style>
  .dialog-backdrop {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.5);
    z-index: 999;
  }

  .dialog {
    position: fixed;
    top: 50%;
    left: 50%;
    transform: translate(-50%, -50%);
    background: var(--bg-panel);
    border: 1px solid var(--border);
    border-radius: 8px;
    box-shadow: 0 10px 40px rgba(0, 0, 0, 0.3);
    z-index: 1000;
    min-width: 320px;
    max-width: 500px;
  }

  .dialog-header {
    padding: 16px;
    border-bottom: 1px solid var(--border);
  }

  .dialog-header h2 {
    margin: 0;
    font-size: calc(16px * var(--app-font-scale));
    font-weight: 600;
    color: var(--text);
  }

  .dialog-body {
    padding: 16px;
    display: flex;
    flex-direction: column;
    gap: 8px;
  }

  .dialog-body label {
    font-size: calc(12px * var(--app-font-scale));
    font-weight: 500;
    color: var(--text-muted);
    text-transform: uppercase;
    letter-spacing: 0.5px;
  }

  .dialog-body input {
    background: var(--bg-input);
    border: 1px solid var(--border);
    color: var(--text);
    padding: 8px 12px;
    border-radius: 4px;
    font-size: calc(13px * var(--app-font-scale));
    font-family: inherit;
  }

  .dialog-body input:focus {
    outline: none;
    border-color: var(--accent);
    box-shadow: 0 0 0 3px var(--accent-alpha, rgba(88, 166, 255, 0.1));
  }

  .dialog-footer {
    padding: 12px 16px;
    border-top: 1px solid var(--border);
    display: flex;
    gap: 8px;
    justify-content: flex-end;
  }

  .btn-cancel,
  .btn-save {
    padding: 6px 16px;
    border-radius: 4px;
    font-size: calc(12px * var(--app-font-scale));
    font-weight: 500;
    cursor: pointer;
    border: 1px solid transparent;
    transition: all 0.2s;
  }

  .btn-cancel {
    background: var(--bg-hover, rgba(255, 255, 255, 0.07));
    color: var(--text);
    border-color: var(--border);
  }

  .btn-cancel:hover {
    background: var(--border);
  }

  .btn-save {
    background: var(--accent);
    color: #fff;
    border-color: var(--accent);
  }

  .btn-save:hover:not(:disabled) {
    background: var(--accent-hover);
  }

  .btn-save:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
</style>
