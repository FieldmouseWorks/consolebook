<script lang="ts">
    import { onMount } from 'svelte';
    import { invalidateAll } from '$app/navigation';
    import { ApiError, listUsers, type UserSummary } from '$lib/api';
    import { authorityHistory, setAuthority, type AuthorityEvent } from '$lib/api/retention';
    import { instant } from '$lib/format';
    let users: UserSummary[] = $state([]);
    let events: AuthorityEvent[] = $state([]);
    let userId = $state(0);
    let granted = $state(true);
    let reason = $state('');
    let busy = $state(false);
    let error = $state('');
    let message = $state('');
    async function reload() { const [roster, history] = await Promise.all([listUsers(), authorityHistory()]); users = roster.users; events = history; }
    onMount(() => { reload().catch(() => { error = 'Authority administration could not be loaded.'; }); });
    function name(id: number) { return users.find(u => u.id === id)?.display_name ?? `User ${id}`; }
    async function save(event: SubmitEvent) {
        event.preventDefault(); busy = true; error = ''; message = '';
        try { await setAuthority(userId, granted, reason); await reload(); await invalidateAll(); reason = ''; message = granted ? 'Retention authority granted.' : 'Retention authority revoked.'; }
        catch (err) { error = err instanceof ApiError ? [err.message, ...err.problems].join('. ') : 'The server could not be reached.'; }
        finally { busy = false; }
    }
</script>
<section class="panel" aria-label="Retention authority">
    <h2>Retention authority</h2>
    <p>Grant or revoke permission to administer policies and holds. This is a separate grant; no role receives it automatically. It does not grant permission to destroy records.</p>
    <form onsubmit={save}>
        <label for="authority-user">User</label>
        <select id="authority-user" bind:value={userId} disabled={busy} required><option value={0} disabled>Select a user</option>{#each users as user}<option value={user.id}>{user.display_name} ({user.username}){user.capabilities.includes('manage_retention') ? ' — retention authority held' : ''}</option>{/each}</select>
        <label for="authority-change">Authority change</label>
        <select id="authority-change" bind:value={granted} disabled={busy}><option value={true}>Grant retention administration</option><option value={false}>Revoke retention administration</option></select>
        <label for="authority-reason">Reason for authority change</label>
        <textarea id="authority-reason" bind:value={reason} required maxlength="1000" disabled={busy}></textarea>
        {#if error}<p class="error" role="alert">{error}</p>{/if}
        {#if message}<p role="status">{message}</p>{/if}
        <button disabled={busy || userId === 0}>Save authority change</button>
    </form>
    <details><summary>Authority history ({events.length})</summary>
        {#each events as event}<p>{instant(event.recorded_at)} · {name(event.actor_user_id)} {event.granted ? 'granted' : 'revoked'} authority for {name(event.user_id)}. {event.reason}</p>{/each}
    </details>
</section>
