<script>
  export let query = '';
  export let onSearch;
  export let onClear;

  let inputValue = query;
  let debounceTimer;

  function scheduleSearch() {
    clearTimeout(debounceTimer);
    if (inputValue.trim()) {
      debounceTimer = setTimeout(() => onSearch(inputValue.trim()), 300);
    } else {
      onClear();
    }
  }

  function handleKeydown(e) {
    if (e.key === 'Enter') {
      clearTimeout(debounceTimer);
      const v = inputValue.trim();
      if (v) onSearch(v);
      else onClear();
    }
  }

  function handleClear() {
    clearTimeout(debounceTimer);
    inputValue = '';
    onClear();
  }
</script>

<div class="search-bar">
  <input
    type="search"
    placeholder="Search contacts…"
    bind:value={inputValue}
    on:input={scheduleSearch}
    on:keydown={handleKeydown}
  />
  {#if inputValue}
    <button class="clear" on:click={handleClear} aria-label="Clear">✕</button>
  {/if}
</div>

<style>
  .search-bar {
    display: flex;
    align-items: center;
    position: relative;
    flex: 1;
    max-width: 400px;
  }

  input {
    width: 100%;
    padding: 0.45rem 2rem 0.45rem 0.7rem;
    border: 1px solid #ccc;
    border-radius: 6px;
    font-size: 0.95rem;
  }

  input:focus {
    outline: 2px solid #2563eb;
    border-color: transparent;
  }

  .clear {
    position: absolute;
    right: 0.5rem;
    background: none;
    border: none;
    cursor: pointer;
    color: #888;
    font-size: 0.8rem;
    padding: 0;
    line-height: 1;
  }

  .clear:hover {
    color: #333;
  }
</style>
