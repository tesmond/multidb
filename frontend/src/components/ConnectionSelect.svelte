<script lang="ts">
  import { onMount } from 'svelte';

  type ConnectionOption = {
    value: string;
    label: string;
  };

  export let value = '';
  export let options: ConnectionOption[] = [];
  export let placeholder = '— select connection —';
  export let disabled = false;
  export let onchange: ((value: string) => void) | undefined = undefined;

  let rootEl: HTMLDivElement | null = null;
  let buttonEl: HTMLButtonElement | null = null;
  let open = false;
  let activeIndex = -1;
  let selectedOption: ConnectionOption | null = null;
  let triggerLabel = placeholder;

  $: selectedOption = options.find((option) => option.value === value) ?? null;
  $: triggerLabel = selectedOption?.label ?? placeholder;

  function closeMenu() {
    open = false;
  }

  function openMenu() {
    if (disabled || options.length === 0) return;
    open = true;
    const currentIndex = options.findIndex((option) => option.value === value);
    activeIndex = currentIndex >= 0 ? currentIndex : 0;
  }

  function toggleMenu() {
    if (open) closeMenu();
    else openMenu();
  }

  function selectOption(option: ConnectionOption) {
    if (disabled) return;
    value = option.value;
    onchange?.(option.value);
    closeMenu();
    buttonEl?.focus();
  }

  function moveActive(delta: number) {
    if (options.length === 0) return;

    if (activeIndex < 0) {
      activeIndex = 0;
      return;
    }

    activeIndex = (activeIndex + delta + options.length) % options.length;
  }

  function handleKeydown(event: KeyboardEvent) {
    if (disabled) return;

    if (!open) {
      if (event.key === 'ArrowDown' || event.key === 'ArrowUp' || event.key === 'Enter' || event.key === ' ') {
        event.preventDefault();
        openMenu();
      }
      return;
    }

    if (event.key === 'Escape') {
      event.preventDefault();
      closeMenu();
      buttonEl?.focus();
      return;
    }

    if (event.key === 'ArrowDown') {
      event.preventDefault();
      moveActive(1);
      return;
    }

    if (event.key === 'ArrowUp') {
      event.preventDefault();
      moveActive(-1);
      return;
    }

    if (event.key === 'Enter' || event.key === ' ') {
      event.preventDefault();
      const option = options[activeIndex] ?? selectedOption ?? options[0];
      if (option) selectOption(option);
    }
  }

  function handleDocumentPointerDown(event: PointerEvent) {
    if (!open || !rootEl) return;
    if (!rootEl.contains(event.target as Node)) {
      closeMenu();
    }
  }

  onMount(() => {
    document.addEventListener('pointerdown', handleDocumentPointerDown, true);
    return () => {
      document.removeEventListener('pointerdown', handleDocumentPointerDown, true);
    };
  });
</script>

<div class="dropdown" bind:this={rootEl}>
  <button
    bind:this={buttonEl}
    type="button"
    class="dropdown-trigger"
    class:is-open={open}
    class:has-selection={!!selectedOption}
    aria-haspopup="listbox"
    aria-expanded={open}
    aria-label="Connection selector"
    disabled={disabled}
    on:click={toggleMenu}
    on:keydown={handleKeydown}
  >
    <span class="dropdown-label">{triggerLabel}</span>
    <span class="dropdown-caret" aria-hidden="true">▾</span>
  </button>

  {#if open}
    <div class="dropdown-menu" role="listbox" tabindex="-1">
      {#each options as option, index}
        <button
          type="button"
          class="dropdown-option"
          class:selected={option.value === value}
          class:active={index === activeIndex}
          role="option"
          aria-selected={option.value === value}
          on:click={() => selectOption(option)}
          on:mouseenter={() => (activeIndex = index)}
        >
          <span class="option-label">{option.label}</span>
          {#if option.value === value}
            <span class="option-check" aria-hidden="true">✓</span>
          {/if}
        </button>
      {/each}
    </div>
  {/if}
</div>

<style>
  .dropdown {
    position: relative;
    display: inline-flex;
    min-width: 180px;
    flex: 0 0 auto;
  }

  .dropdown-trigger {
    width: 100%;
    display: inline-flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 5px 10px;
    border-radius: 4px;
    border: 1px solid var(--border);
    background: var(--bg-input);
    color: var(--text);
    font-size: 12px;
    cursor: pointer;
    text-align: left;
  }

  .dropdown-trigger:hover:not(:disabled),
  .dropdown-trigger.is-open {
    border-color: var(--accent);
  }

  .dropdown-trigger:focus {
    outline: none;
    border-color: var(--accent);
  }

  .dropdown-trigger:disabled {
    opacity: 0.55;
    cursor: not-allowed;
  }

  .dropdown-label {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .dropdown-caret {
    flex: 0 0 auto;
    opacity: 0.8;
    font-size: 14px;
    line-height: 1;
  }

  .dropdown-menu {
    position: absolute;
    left: 0;
    top: calc(100% + 4px);
    z-index: 40;
    min-width: 100%;
    padding: 4px;
    border: 1px solid var(--border);
    border-radius: 8px;
    background: var(--bg-panel, #151821);
    box-shadow: 0 14px 40px rgba(0, 0, 0, 0.32);
    max-height: 260px;
    overflow: auto;
  }

  .dropdown-option {
    width: 100%;
    display: flex;
    align-items: center;
    justify-content: space-between;
    gap: 12px;
    padding: 8px 10px;
    border: 0;
    border-radius: 6px;
    background: transparent;
    color: var(--text);
    font-size: 12px;
    cursor: pointer;
    text-align: left;
  }

  .dropdown-option:hover,
  .dropdown-option.active {
    background: var(--bg-hover, rgba(255, 255, 255, 0.08));
  }

  .dropdown-option.selected {
    background: rgba(255, 255, 255, 0.08);
    box-shadow: inset 0 0 0 1px var(--accent);
  }

  .option-label {
    min-width: 0;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .option-check {
    flex: 0 0 auto;
    color: var(--accent);
    font-size: 11px;
  }
</style>