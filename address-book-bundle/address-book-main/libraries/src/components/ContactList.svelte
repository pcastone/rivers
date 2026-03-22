<script>
  export let contacts = [];

  function initials(contact) {
    const f = contact.first_name?.[0] ?? '';
    const l = contact.last_name?.[0] ?? '';
    return (f + l).toUpperCase() || '?';
  }
</script>

{#if contacts.length === 0}
  <p class="empty">No contacts found.</p>
{:else}
  <div class="grid">
    {#each contacts as contact (contact.id)}
      <div class="card">
        <div class="avatar">
          {#if contact.avatar_url}
            <img src={contact.avatar_url} alt="{contact.first_name} {contact.last_name}" />
          {:else}
            <span class="initials">{initials(contact)}</span>
          {/if}
        </div>
        <div class="info">
          <strong>{contact.first_name} {contact.last_name}</strong>
          <a href="mailto:{contact.email}">{contact.email}</a>
          {#if contact.phone}<span>{contact.phone}</span>{/if}
          {#if contact.company}<span class="muted">{contact.company}</span>{/if}
          {#if contact.city || contact.state}
            <span class="muted">
              {[contact.city, contact.state].filter(Boolean).join(', ')}
            </span>
          {/if}
        </div>
      </div>
    {/each}
  </div>
{/if}

<style>
  .empty {
    text-align: center;
    color: #888;
    padding: 2rem;
  }

  .grid {
    display: grid;
    grid-template-columns: repeat(auto-fill, minmax(260px, 1fr));
    gap: 1rem;
  }

  .card {
    display: flex;
    gap: 0.75rem;
    padding: 0.9rem;
    border: 1px solid #e0e0e0;
    border-radius: 8px;
    background: #fff;
  }

  .avatar {
    flex-shrink: 0;
    width: 48px;
    height: 48px;
    border-radius: 50%;
    overflow: hidden;
    background: #ddd;
    display: flex;
    align-items: center;
    justify-content: center;
  }

  .avatar img {
    width: 100%;
    height: 100%;
    object-fit: cover;
  }

  .initials {
    font-size: 1.1rem;
    font-weight: 600;
    color: #555;
  }

  .info {
    display: flex;
    flex-direction: column;
    gap: 0.2rem;
    min-width: 0;
    font-size: 0.85rem;
  }

  .info strong {
    font-size: 0.95rem;
  }

  .info a {
    color: #2563eb;
    text-decoration: none;
    overflow: hidden;
    text-overflow: ellipsis;
    white-space: nowrap;
  }

  .muted {
    color: #777;
  }
</style>
