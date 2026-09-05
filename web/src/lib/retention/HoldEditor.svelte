<script lang="ts">
    import { ApiError } from '$lib/api/transport';
    import { saveHold, holdLabels, type Hold, type HoldKind, type HoldScope, type ScopeOptions } from '$lib/api/retention';
    let { previous = null, options, onSaved, onCancel }: { previous?: Hold | null; options: ScopeOptions; onSaved: () => Promise<void>; onCancel: () => void } = $props();
    let scopeKind = $state<HoldScope['kind']>('installation');
    let scopeId = $state(0);
    let kind = $state<HoldKind>('litigation');
    let authority = $state('');
    let reason = $state('');
    let busy = $state(false);
    let error = $state('');
    import { onMount } from 'svelte';
    onMount(() => {
        if (previous) {
            scopeKind = previous.scope.kind;
            scopeId = previous.scope.kind === 'record' ? previous.scope.record_id : previous.scope.kind === 'enrollment' ? previous.scope.enrollment_id : 0;
            kind = previous.kind; authority = previous.authority;
        }
    });
    async function save(event: SubmitEvent) {
        event.preventDefault(); busy = true; error = '';
        const scope: HoldScope = scopeKind === 'installation' ? { kind: 'installation' } : scopeKind === 'enrollment' ? { kind: 'enrollment', enrollment_id: scopeId } : { kind: 'record', record_id: scopeId };
        try { await saveHold({ scope, kind, authority, reason }, previous?.id ?? null); await onSaved(); onCancel(); }
        catch (err) { error = err instanceof ApiError ? [err.message, ...err.problems].join('. ') : 'The server could not be reached.'; }
        finally { busy = false; }
    }
</script>
<form class="card" onsubmit={save}>
    <h2>{previous ? `Replace hold ${previous.id}` : 'New hold'}</h2>
    <p>{previous ? 'Saving releases the previous scope and activates this replacement together. The previous history remains available.' : 'Holds remain active until explicitly released. There is no automatic expiry.'}</p>
    <label for="hold-scope">Hold scope</label>
    <select id="hold-scope" bind:value={scopeKind} onchange={() => scopeId = 0} disabled={busy}><option value="installation">Entire installation</option><option value="enrollment">One enrollment</option><option value="record">One record</option></select>
    {#if scopeKind !== 'installation'}
        <label for="hold-target">{scopeKind === 'enrollment' ? 'Enrollment' : 'Record'}</label>
        <select id="hold-target" bind:value={scopeId} required disabled={busy}>
            <option value={0} disabled>Select the exact scope</option>
            {#each scopeKind === 'enrollment' ? options.enrollments : options.records as option}<option value={option.id}>{option.label}</option>{/each}
        </select>
    {/if}
    <label for="hold-kind">Hold kind</label>
    <select id="hold-kind" bind:value={kind} disabled={busy}>{#each Object.entries(holdLabels) as [value, label]}<option {value}>{label}</option>{/each}</select>
    <label for="hold-authority">Hold authority reference</label>
    <input id="hold-authority" bind:value={authority} required maxlength="200" disabled={busy} />
    <label for="hold-reason">{previous ? 'Reason for replacement' : 'Reason for hold'}</label>
    <textarea id="hold-reason" bind:value={reason} required maxlength="1000" disabled={busy}></textarea>
    {#if error}<p class="error" role="alert">{error}</p>{/if}
    <div class="row"><button disabled={busy || (scopeKind !== 'installation' && scopeId === 0)}>{previous ? 'Save replacement hold' : 'Place hold'}</button><button type="button" class="secondary" disabled={busy} onclick={onCancel}>Cancel</button></div>
</form>
