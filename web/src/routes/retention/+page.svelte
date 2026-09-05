<script lang="ts">
    import { ApiError } from '$lib/api/transport';
    import { listPolicies, listHolds, listScopes, releaseHold, recordHolds, classLabels, triggerLabels, holdLabels, scopeLabel, type Policy, type Hold, type ScopeOptions } from '$lib/api/retention';
    import PolicyEditor from '$lib/retention/PolicyEditor.svelte';
    import HoldEditor from '$lib/retention/HoldEditor.svelte';
    import AuthorityPanel from '$lib/retention/AuthorityPanel.svelte';
    import { instant } from '$lib/format';
    import type { ShellData } from '../+layout';
    let { data }: { data: ShellData } = $props();
    let canManage = $derived(data.session?.capabilities.includes('manage_retention') ?? false);
    let canGrant = $derived(data.session?.capabilities.includes('manage_users') ?? false);
    let policies: Policy[] = $state([]);
    let holds: Hold[] = $state([]);
    let options: ScopeOptions = $state({ enrollments: [], records: [] });
    let loaded = $state(false);
    let error = $state('');
    let message = $state('');
    let editor = $state<{ previous: Hold | null } | null>(null);
    let releasing: Hold | null = $state(null);
    let releaseReason = $state('');
    let busy = $state(false);
    let recordId = $state(0);
    let matched: Hold[] | null = $state(null);
    function describe(err: unknown) { return err instanceof ApiError ? [err.message, ...err.problems].join('. ') : 'The server could not be reached.'; }
    async function refresh() {
        const [p, h, o] = await Promise.all([listPolicies(), listHolds(), listScopes()]);
        policies = p; holds = h; options = o; loaded = true; matched = null;
    }
    $effect(() => {
        if (canManage) {
            refresh().catch(err => { error = describe(err); });
        } else { policies = []; holds = []; options = { enrollments: [], records: [] }; loaded = false; matched = null; editor = null; releasing = null; }
    });
    async function release(event: SubmitEvent) {
        event.preventDefault(); if (!releasing) return;
        busy = true; error = ''; message = '';
        try { await releaseHold(releasing.id, releaseReason); releasing = null; releaseReason = ''; await refresh(); message = 'Hold released. Its history remains available.'; }
        catch (err) { error = describe(err); } finally { busy = false; }
    }
    async function lookup(event: SubmitEvent) {
        event.preventDefault(); busy = true; error = ''; matched = null;
        try { matched = await recordHolds(recordId); }
        catch (err) { error = describe(err); } finally { busy = false; }
    }
</script>
<h1>Retention and holds</h1>
<p class="lede">Configure your agency’s approved schedule and preserve records under holds.</p>
<p>Disposition execution is not available in this release. Policies and hold lookup do not authorize deletion. No records are removed by these controls.</p>
{#if canGrant}<AuthorityPanel />{/if}
{#if error}<p class="error" role="alert">{error}</p>{/if}
{#if message}<p role="status">{message}</p>{/if}
{#if !canManage}
    <p role="status">Explicit retention administration authority is required to view policies and holds.</p>
{:else if !loaded}
    <p>Loading retention administration…</p>
{:else}
    <PolicyEditor {policies} onSaved={refresh} />
    <section class="panel" aria-label="Policy history">
        <h2>Policy history</h2>
        {#if policies.length === 0}<p>No retention policies configured. Missing policy never permits destruction.</p>{/if}
        {#each policies as policy}
            <article>
                <h3>{classLabels[policy.record_class]} · version {policy.version_number} {policies.find(p => p.record_class === policy.record_class)?.id === policy.id ? '(current)' : '(superseded)'}</h3>
                <p>{policy.authority} · {policy.action === 'retain' ? 'Retain; no destruction authorized' : `Destroy after ${policy.retention_days} elapsed days from ${triggerLabels[policy.retention_trigger].toLowerCase()}, subject to holds and confirmation`}.</p>
                <p>{policy.reason}</p><p>Recorded by user {policy.created_by} · {instant(policy.created_at)}</p>
            </article>
        {/each}
    </section>
    <section class="panel" aria-label="Hold history">
        <h2>Hold history</h2>
        <button onclick={() => { editor = { previous: null }; releasing = null; }}>New hold</button>
        {#if holds.length === 0}<p>No holds recorded.</p>{/if}
        {#each holds as hold}
            <article>
                <h3>Hold {hold.id} · {hold.release ? 'Released' : 'Active'} · {holdLabels[hold.kind]}</h3>
                <p>{scopeLabel(hold.scope, options)} · {hold.authority}</p>
                <p>{hold.reason}</p><p>Created by user {hold.created_by} · {instant(hold.created_at)}{hold.replaces_id ? ` · replaces hold ${hold.replaces_id}` : ''}</p>
                {#if hold.release}<p>Released by user {hold.release.released_by} · {instant(hold.release.released_at)}{hold.release.replacement_id ? ` · replaced by hold ${hold.release.replacement_id}` : ''}. {hold.release.reason}</p>
                {:else}<div class="row"><button class="secondary small" onclick={() => { editor = { previous: hold }; releasing = null; }}>Replace hold {hold.id}</button><button class="secondary small" onclick={() => { releasing = hold; releaseReason = ''; editor = null; }}>Release hold {hold.id}</button></div>{/if}
            </article>
        {/each}
    </section>
    {#if editor}{#key editor}<HoldEditor previous={editor.previous} {options} onSaved={refresh} onCancel={() => editor = null} />{/key}{/if}
    {#if releasing}
        <form class="card" onsubmit={release}>
            <h2>Release hold {releasing.id}</h2><p>{scopeLabel(releasing.scope, options)} · {releasing.authority}</p>
            <label for="release-reason">Reason for release</label><textarea id="release-reason" bind:value={releaseReason} required maxlength="1000" disabled={busy}></textarea>
            <div class="row"><button disabled={busy}>Confirm hold release</button><button type="button" class="secondary" disabled={busy} onclick={() => releasing = null}>Cancel</button></div>
        </form>
    {/if}
    <form class="card" onsubmit={lookup}>
        <h2>Check a record’s holds</h2><label for="lookup-record">Record to check</label>
        <select id="lookup-record" bind:value={recordId} onchange={() => matched = null} disabled={busy}><option value={0} disabled>Select a record</option>{#each options.records as record}<option value={record.id}>{record.label}</option>{/each}</select>
        <button disabled={busy || recordId === 0}>Check active holds</button>
        {#if matched !== null}
            <p role="status">{matched.length} applicable active holds. This checks holds only; it is not a disposition eligibility decision.</p>
            {#each matched as hold}<p>Hold {hold.id} · {holdLabels[hold.kind]} · {scopeLabel(hold.scope, options)} · {hold.authority}</p>{/each}
        {/if}
    </form>
{/if}
<style>
    h2 { font-size: 1.1rem; }
    h3 { font-size: 1rem; margin-bottom: .4rem; }
    article { border-top: 1px solid light-dark(#d8dce3, #303642); margin-top: 1rem; padding-top: .5rem; overflow-wrap: anywhere; }
</style>
