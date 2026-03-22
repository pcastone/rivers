<script>
  import ContactList from './components/ContactList.svelte';
  import SearchBar from './components/SearchBar.svelte';

  let mode = 'list';
  let contacts = [];
  let total = 0;
  let page = 0;
  let pageSize = 20;
  let query = '';
  let loading = false;
  let error = null;

  async function fetchContacts() {
    loading = true;
    error = null;
    try {
      const res = await fetch(`/api/contacts?limit=${pageSize}&offset=${page * pageSize}`);
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      const json = await res.json();
      contacts = json.data ?? [];
      total = json.meta?.total ?? contacts.length;
    } catch (e) {
      error = e.message;
      contacts = [];
    } finally {
      loading = false;
    }
  }

  async function fetchSearch(q) {
    loading = true;
    error = null;
    try {
      const res = await fetch(`/api/contacts/search?q=${encodeURIComponent(q)}&limit=50`);
      if (!res.ok) throw new Error(`HTTP ${res.status}`);
      const json = await res.json();
      contacts = json.data ?? [];
      total = contacts.length;
    } catch (e) {
      error = e.message;
      contacts = [];
    } finally {
      loading = false;
    }
  }

  function handleSearch(q) {
    query = q;
    mode = 'search';
    page = 0;
    fetchSearch(q);
  }

  function handleClear() {
    query = '';
    mode = 'list';
    page = 0;
    fetchContacts();
  }

  function prevPage() {
    if (page > 0) {
      page -= 1;
      fetchContacts();
    }
  }

  function nextPage() {
    if ((page + 1) * pageSize < total) {
      page += 1;
      fetchContacts();
    }
  }

  const totalPages = () => Math.max(1, Math.ceil(total / pageSize));

  // Initial load
  fetchContacts();
</script>

<main>
  <header>
    <h1>Address Book</h1>
    <SearchBar {query} onSearch={handleSearch} onClear={handleClear} />
  </header>

  {#if loading}
    <div class="status">Loading…</div>
  {:else if error}
    <div class="status error">Error: {error}</div>
  {:else}
    <ContactList {contacts} />

    {#if mode === 'list'}
      <nav class="pagination">
        <button on:click={prevPage} disabled={page === 0}>← Prev</button>
        <span>Page {page + 1} of {totalPages()}</span>
        <button on:click={nextPage} disabled={(page + 1) * pageSize >= total}>Next →</button>
      </nav>
    {/if}
  {/if}
</main>

<style>
  main {
    font-family: system-ui, sans-serif;
    max-width: 960px;
    margin: 0 auto;
    padding: 1.5rem;
  }

  header {
    display: flex;
    align-items: center;
    gap: 1rem;
    margin-bottom: 1.5rem;
    flex-wrap: wrap;
  }

  h1 {
    margin: 0;
    font-size: 1.5rem;
    white-space: nowrap;
  }

  .status {
    text-align: center;
    padding: 2rem;
    color: #666;
  }

  .status.error {
    color: #c0392b;
  }

  .pagination {
    display: flex;
    align-items: center;
    justify-content: center;
    gap: 1rem;
    margin-top: 1.5rem;
  }

  .pagination button {
    padding: 0.4rem 0.8rem;
    border: 1px solid #ccc;
    border-radius: 4px;
    background: white;
    cursor: pointer;
  }

  .pagination button:disabled {
    opacity: 0.4;
    cursor: default;
  }
</style>
