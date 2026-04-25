<script lang="ts">
  import { showImportDialog, importDialogConnId, statusMessage, requestSchemaRefresh } from '../stores/appStore';
  import { ImportTable, SelectImportFile } from '../../wailsjs/go/main/App';

  let importType = 'zipped-sql';
  let sourcePath = '';
  let error = '';
  let importing = false;

  function close() {
    showImportDialog.set(false);
    sourcePath = '';
    error = '';
    importType = 'zipped-sql';
  }

  async function browseFile() {
    error = '';
    try {
      const selected = await SelectImportFile(importType);
      if (!selected) {
        return;
      }
      sourcePath = selected;
    } catch (e: any) {
      error = String(e);
    }
  }

  async function handleImport() {
    error = '';
    if (!sourcePath) {
      error = 'Please choose a file to import.';
      return;
    }
    importing = true;
    try {
      await ImportTable($importDialogConnId, importType, sourcePath);
      statusMessage.set('Import completed');
      requestSchemaRefresh($importDialogConnId);
      close();
    } catch (e: any) {
      error = String(e);
    } finally {
      importing = false;
    }
  }
</script>

{#if $showImportDialog}
<div class="modal-overlay" on:click|self={close} on:keydown={e => e.key === 'Escape' && close()} role="dialog" aria-modal="true" aria-label="Import Table">
  <div class="modal">
    <div class="modal-header">
      <h2>Import Table</h2>
      <button class="close-btn" on:click={close} aria-label="Close">✕</button>
    </div>

    <div class="modal-body">
      <div class="form-row">
        <label for="import-type">Import format</label>
        <select id="import-type" bind:value={importType}>
          <option value="zipped-sql">Import zipped sql</option>
          <option value="pgdump">Import from pgdump</option>
        </select>
      </div>

      <div class="form-row">
        <label for="import-file-path">File</label>
        <div class="file-row">
          <button class="btn-secondary" type="button" on:click={browseFile}>Browse...</button>
          <input id="import-file-path" type="text" readonly value={sourcePath} placeholder="No file selected" />
        </div>
      </div>

      {#if error}
        <p class="error">{error}</p>
      {/if}
    </div>

    <div class="modal-footer">
      <button class="btn-secondary" type="button" on:click={close} disabled={importing}>Cancel</button>
      <button class="btn-primary" type="button" on:click={handleImport} disabled={importing || !sourcePath}>
        {importing ? 'Importing…' : 'Import'}
      </button>
    </div>
  </div>
</div>
{/if}

<style>
  .modal-overlay {
    position: fixed;
    inset: 0;
    background: rgba(0, 0, 0, 0.6);
    display: flex;
    align-items: center;
    justify-content: center;
    z-index: 200;
  }
  .modal {
    background: var(--bg-surface);
    border: 1px solid var(--border);
    border-radius: 8px;
    width: 480px;
    max-width: 95vw;
    box-shadow: 0 8px 32px rgba(0, 0, 0, 0.5);
  }
  .modal-header {
    display: flex;
    align-items: center;
    justify-content: space-between;
    padding: 16px 20px;
    border-bottom: 1px solid var(--border);
  }
  .modal-header h2 {
    margin: 0;
    font-size: 16px;
    font-weight: 600;
  }
  .close-btn {
    background: none;
    border: none;
    color: var(--text-muted);
    cursor: pointer;
    font-size: 16px;
    padding: 4px 8px;
  }
  .close-btn:hover {
    color: var(--text);
  }
  .modal-body {
    padding: 20px;
    display: flex;
    flex-direction: column;
    gap: 12px;
  }
  .form-row {
    display: flex;
    flex-direction: column;
    gap: 4px;
  }
  label {
    display: flex;
    flex-direction: column;
    gap: 4px;
    font-size: 12px;
    color: var(--text-muted);
  }
  input,
  select {
    background: var(--bg-input);
    border: 1px solid var(--border);
    color: var(--text);
    padding: 7px 10px;
    border-radius: 4px;
    font-size: 13px;
    width: 100%;
    box-sizing: border-box;
  }
  input:focus,
  select:focus {
    outline: none;
    border-color: var(--accent);
  }
  .file-row {
    display: grid;
    grid-template-columns: auto 1fr;
    gap: 8px;
    align-items: center;
  }
  .file-row input {
    min-width: 0;
  }
  .modal-footer {
    display: flex;
    justify-content: flex-end;
    gap: 8px;
    padding: 16px 20px;
    border-top: 1px solid var(--border);
  }
  .btn-primary,
  .btn-secondary {
    padding: 7px 16px;
    border-radius: 4px;
    font-size: 13px;
    cursor: pointer;
    border: 1px solid transparent;
  }
  .btn-primary {
    background: var(--accent);
    color: #fff;
    border-color: var(--accent);
  }
  .btn-primary:hover {
    background: var(--accent-hover);
  }
  .btn-primary:disabled,
  .btn-secondary:disabled {
    opacity: 0.5;
    cursor: not-allowed;
  }
  .btn-secondary {
    background: var(--bg-input);
    color: var(--text);
    border-color: var(--border);
  }
  .btn-secondary:hover {
    border-color: var(--accent);
  }
  .error {
    color: var(--error);
    font-size: 12px;
    margin: 0;
  }
</style>
