<script lang="ts">
    import { ApiError } from '$lib/api/transport';
    import { savePolicy, classLabels, triggerLabels, type Policy, type RecordClass, type RetentionTrigger, type RetentionAction } from '$lib/api/retention';
    let { policies, onSaved }: { policies: Policy[]; onSaved: () => Promise<void> } = $props();
    let recordClass = $state<RecordClass>('daily_report');
    let expected = $state<number | null>(null);
    let authority = $state('');
    let trigger = $state<RetentionTrigger>('finalized_at');
    let days = $state(0);
    let action = $state<RetentionAction>('retain');
    let reason = $state('');
    let error = $state('');
    let message = $state('');
    let busy = $state(false);
    function loadCurrent() {
        const current = policies.find(p => p.record_class === recordClass);
        expected = current?.id ?? null;
        authority = current?.authority ?? '';
        trigger = current?.retention_trigger ?? (recordClass === 'disposition_event' ? 'disposed_at' : 'finalized_at');
        days = current?.retention_days ?? 0;
        action = current?.action ?? 'retain';
        reason = '';
        message = '';
    }
    // Remount after initial load gives a deliberate editing baseline. Later
    // background refreshes never silently advance the expected policy version.
    import { onMount } from 'svelte';
    onMount(loadCurrent);
    async function reload() {
        busy = true; error = "";
        try { await onSaved(); loadCurrent(); }
        catch (err) { error = err instanceof ApiError ? err.message : "The server could not be reached."; }
        finally { busy = false; }
    }
    async function save(event: SubmitEvent) {
        event.preventDefault(); busy = true; error = ''; message = '';
        try {
            await savePolicy({ record_class: recordClass, expected_current_id: expected, authority, retention_trigger: trigger, retention_days: action === 'retain' ? 0 : days, action, reason });
            await onSaved(); loadCurrent(); message = 'Policy version saved. No records were deleted.';
        } catch (err) {
            error = err instanceof ApiError ? [err.message, ...err.problems].join('. ') : 'The server could not be reached.';
        } finally { busy = false; }
    }
</script>
<form class="card" onsubmit={save}>
    <h2>Policy version</h2>
    <p>Enter your agency’s approved schedule. A new version preserves every earlier version.</p>
    <label for="policy-class">Record class</label>
    <select id="policy-class" bind:value={recordClass} onchange={loadCurrent} disabled={busy}>
        {#each Object.entries(classLabels) as [value, label]}<option {value}>{label}</option>{/each}
    </select>
    <p>{expected === null ? 'No current policy for this class.' : `Replaces policy ${expected}.`}</p>
    <label for="policy-authority">Disposition authority reference</label>
    <input id="policy-authority" bind:value={authority} required maxlength="200" disabled={busy} />
    <label for="policy-trigger">Retention starts at</label>
    <select id="policy-trigger" bind:value={trigger} disabled={busy}>
        {#each Object.entries(triggerLabels).filter(([value]) => recordClass === 'disposition_event' ? value === 'disposed_at' : value !== 'disposed_at') as [value, label]}<option {value}>{label}</option>{/each}
    </select>
    <label for="policy-action">Scheduled action</label>
    <select id="policy-action" bind:value={action} disabled={busy}><option value="retain">Retain — no destruction authorized</option><option value="destroy">Destroy after the minimum period, subject to holds and confirmation</option></select>
    {#if action === 'destroy'}
        <label for="policy-days">Minimum retention (elapsed days of 24 hours)</label>
        <input id="policy-days" type="number" min="0" max="365250" step="1" bind:value={days} required disabled={busy} />
    {/if}
    <label for="policy-reason">Reason for this version</label>
    <textarea id="policy-reason" bind:value={reason} required maxlength="1000" disabled={busy}></textarea>
    {#if error}<p class="error" role="alert">{error}</p>{/if}
    {#if message}<p role="status">{message}</p>{/if}
    <div class="row"><button disabled={busy}>Save policy version</button><button type="button" class="secondary" onclick={reload} disabled={busy}>Load current policy</button></div>
</form>
