// One owner for the draft working copy an author edits: the editable
// values, the revision every save carries, the debounced autosave chain,
// the metadata refresh, and the recovery buffer that keeps a refused save's
// text (#34; ADR 0008; ownership boundary of #59).
//
// The route page keeps loading, review, finalization, acknowledgment, and
// amendment actions and reads this controller's content; it never holds a
// second copy of an editable value. The controller takes the draft's
// server view as a getter, so nothing is mirrored across two owners.
//
// Two lifetimes live here, and they are deliberately different:
//
// - component/controller-lifetimed state: `values`, `notObserved`,
//   `modifiers`, `narratives`, `revision`, and the save chain. Discarded
//   when the controller is destroyed;
// - `refused`: every refused save's divergent text that the author has not
//   discarded yet, newest first, and only until they discard it, the
//   controller is destroyed (navigation or logout), or a new draft
//   identity gets a new controller. It is never persisted anywhere — no
//   localStorage, sessionStorage, IndexedDB, URL state, log, or server
//   call — because the refusal is a concurrency answer, not record content
//   (issue #34).
import { getDraft, saveDraftContent, type DraftContent, type DraftView } from '$lib/api';
import { ApiError } from '$lib/api/transport';

/** The debounced autosave's delay once an edit lands. */
const AUTOSAVE_DELAY_MS = 600;

/** Whether a save is pending, running, or settled. */
export type SaveState = 'idle' | 'pending' | 'saving' | 'saved' | 'failed';

/** One narrative as it stood when a refused save was attempted. */
export interface RefusedNarrative {
	form_narrative_id: number;
	label: string;
	text: string;
	/**
	 * The text changed after the refused request was already in flight, so
	 * it was never even submitted. It is kept for the same reason as the
	 * refused text and shown distinctly, never merged.
	 */
	typed_while_pending: boolean;
}

/** One rating as it stood when a refused save was attempted. */
export interface RefusedRating {
	form_competency_id: number;
	name: string;
	not_observed: boolean;
	value: number | null;
	modifier_codes: string[];
	/** The same modifiers as ids, for comparing against the working copy. */
	modifier_ids: number[];
	/** See [`RefusedNarrative::typed_while_pending`]. */
	typed_while_pending: boolean;
}

/**
 * A save the revision contract refused (`stale_save`) and the text it
 * carried, still readable after the workspace reloaded the winner.
 *
 * `narratives` and `ratings` hold only what actually differed from the
 * reloaded content, so the page shows the author's divergent text and
 * never invents a merge (issue #34; ADR 0008: one working copy).
 */
export interface RefusedBuffer {
	/** Ascending `form_narrative_id`: a stable order for the buffer. */
	narratives: RefusedNarrative[];
	ratings: RefusedRating[];
	/**
	 * Whether a later save by the writer carried this buffer's text into
	 * the draft. An unresolved buffer holds a workflow act, so the writer
	 * either puts the text back or discards it; the text itself stays
	 * readable either way.
	 */
	resolved: boolean;
}

/** The durable form labels a refused buffer needs to name its fields. */
interface FormShape {
	narrative_labels: Map<number, string>;
	competency_names: Map<number, string>;
	modifier_codes: Map<number, string>;
}

/** The editable state at one instant, kept durable across a reload. */
export interface EditorSnapshot {
	revision: number;
	values: Map<number, number | null>;
	notObserved: Map<number, boolean>;
	modifiers: Map<number, Set<number>>;
	narratives: Map<number, string>;
	shape: FormShape;
}

/** What one `saveNow` attempt settled as. */
export type SaveResult =
	| { status: 'saved' }
	/** The typed refusal or transport answer the attempt failed with. */
	| { status: 'failed'; message: string }
	| { status: 'ignored' }
	| { status: 'stale'; refused: EditorSnapshot };

interface SaveRun {
	draft_id: number;
	refused: EditorSnapshot | null;
}

/**
 * The mutable draft editor for one draft identity. Construct one per draft
 * identity and call [`destroy`] when the route unmounts: a new draft gets
 * a new controller, so an old draft's in-flight save can never attach its
 * result or its refused text to another draft.
 */
export class DraftEditorController {
	/** The revision every save and workflow act carries. */
	revision = $state(0);
	/** Rating value keyed by `form_competency_id`. */
	values: Record<number, number | null> = $state({});
	/** "No opportunity to observe" keyed by `form_competency_id`. */
	notObserved: Record<number, boolean> = $state({});
	/** Picked modifiers keyed by `form_competency_id`, then modifier id. */
	modifiers: Record<number, Record<number, boolean>> = $state({});
	/** Narrative text keyed by `form_narrative_id`. */
	narratives: Record<number, string> = $state({});
	saveState: SaveState = $state('idle');
	/**
	 * The text each refused save carried, plus anything typed while its
	 * request was still in flight, for the writer to read and copy. Empty
	 * whenever there is nothing to recover, and it only ever holds text
	 * from saves the contract actually refused or from edits that were
	 * never submitted because of a refusal. A later refusal is added, never
	 * substituted: an earlier buffer the author has not copied yet is still
	 * their text.
	 */
	refused: RefusedBuffer[] = $state([]);
	/**
	 * A refusal whose reload did not land: the page's working copy is not
	 * the winner's, so nothing may be acted on until a reload or a save
	 * succeeds. Kept apart from the buffers because it can be true with no
	 * buffer to show.
	 */
	#reloadFailed = $state(false);
	/**
	 * How many refusals this controller has recorded. A save that started
	 * before a refusal cannot resolve it: only a save the writer made after
	 * seeing the refusal carries the text they copied back.
	 */
	#refusals = 0;

	/**
	 * Whether a workflow act must wait: a refusal's text is not in the
	 * draft yet, or the copy the page holds is not the winner's. True only
	 * while this draft's own refusals are unresolved, so the guard is taken
	 * away by a draft identity change along with the buffers, and a save
	 * the writer makes after seeing a refusal resolves exactly the buffers
	 * that save carries — never another refusal's text.
	 */
	get unresolved(): boolean {
		return this.#reloadFailed || this.refused.some((buffer) => !buffer.resolved);
	}

	/**
	 * Every settled save reports here, whichever path started it: the
	 * debounce, an explicit save, or a flush before a workflow act. The
	 * page is the one that reloads the winner, because only it owns the
	 * loaded view.
	 */
	onSettled: (result: SaveResult) => void = () => {};

	readonly #view: () => DraftView | null;
	readonly #mayEdit: (view: DraftView) => boolean;
	#timer: ReturnType<typeof setTimeout> | null = null;
	#inFlight: Promise<void> | null = null;
	#release: (() => void) | null = null;
	#dirty = false;
	#destroyed = false;
	/**
	 * A refusal the page has not answered with a reload yet. A later save
	 * that the same stale revision also refuses reports nothing new: it
	 * carries no text the first refusal did not already hand over, and
	 * reporting again would overwrite the buffer with post-reload state.
	 */
	#reported_stale = false;

	/**
	 * `view` returns the draft the page loaded (or `null` while loading);
	 * `mayEdit` is the page's session-derived editing rule, because the
	 * controller never guesses authority from a role label (ADR 0010).
	 */
	constructor(
		view: () => DraftView | null,
		mayEdit: (view: DraftView) => boolean
	) {
		this.#view = view;
		this.#mayEdit = mayEdit;
	}

	/** Whether the author may edit the loaded draft right now. */
	get editable(): boolean {
		const view = this.#view();
		return view !== null && openForEditing(view) && this.#mayEdit(view);
	}

	// ------------------------------------------------------------ editing

	/** Debounced autosave once an edit lands. */
	scheduleSave(): void {
		if (this.#destroyed || !this.editable) {
			return;
		}
		this.saveState = 'pending';
		if (this.#timer !== null) {
			clearTimeout(this.#timer);
		}
		this.#timer = setTimeout(() => {
			this.#timer = null;
			void this.saveNow();
		}, AUTOSAVE_DELAY_MS);
	}

	/**
	 * Saves the current content, or joins the save already running. A
	 * `stale_save` is a typed answer, not a failure: it is reported with
	 * the text the refused save carried, so the page can reload the winner
	 * while that text stays readable. A real transport or server failure
	 * is reported as `failed`. Never rejects.
	 */
	saveNow(): Promise<void> {
		if (this.#destroyed) {
			return Promise.resolve();
		}
		// The armed timer is this attempt, or — when a run is already in
		// flight — an edit the running chain will re-send. Either way it is
		// consumed here: a timer left armed would fire after a workflow act
		// completed and save over it.
		if (this.#timer !== null) {
			clearTimeout(this.#timer);
			this.#timer = null;
		}
		if (this.#inFlight !== null) {
			// A newer edit arrived while a request is in flight; the
			// running chain re-sends the latest state once it settles.
			this.#dirty = true;
			return this.#inFlight;
		}
		const draft_id = this.#view()?.id ?? null;
		if (draft_id === null) {
			return Promise.resolve();
		}
		if (this.#reported_stale) {
			// The page has not yet reloaded the winner for the refusal it
			// was already told about; a further attempt would only be
			// refused by the same stale revision — and it is not a save
			// that is still coming, so the indicator must not keep saying
			// one is.
			this.saveState = 'idle';
			return Promise.resolve();
		}
		const run: SaveRun = { draft_id, refused: null };
		this.#settled = { status: 'saved' };
		const reported = new Promise<void>((resolve) => {
			this.#release = resolve;
		});
		const work = this.#saveChain(run);
		this.#inFlight = work;
		void work.finally(() => {
			if (this.#inFlight === work) {
				this.#inFlight = null;
			}
			const release = this.#release;
			this.#release = null;
			release?.();
			if (!this.#destroyed) {
				this.onSettled(this.#settled);
			}
		});
		return reported;
	}

	/**
	 * Nothing workflow-shaped runs over unsaved edits: a pending or
	 * in-flight save lands first and reports through `onSettled`, whatever
	 * it settled as.
	 */
	async flush(): Promise<void> {
		if (this.#timer !== null) {
			await this.saveNow();
			return;
		}
		if (this.#inFlight !== null) {
			await this.#inFlight;
		}
	}

	/**
	 * Adopts the loaded working copy, replacing every editable value, and
	 * returns the state it replaced. The page keeps that instead of a
	 * snapshot taken before its reload: text typed while the reload was in
	 * flight is only in the state being replaced (issue #34).
	 */
	adopt(view: DraftView): EditorSnapshot {
		const replaced = this.snapshot();
		const values: Record<number, number | null> = {};
		const notObserved: Record<number, boolean> = {};
		const modifiers: Record<number, Record<number, boolean>> = {};
		for (const competency of view.form.competencies) {
			values[competency.form_competency_id] = null;
			notObserved[competency.form_competency_id] = false;
			modifiers[competency.form_competency_id] = {};
		}
		for (const rating of view.content.ratings) {
			values[rating.form_competency_id] = rating.value;
			notObserved[rating.form_competency_id] = rating.not_observed;
			const picked: Record<number, boolean> = {};
			for (const id of rating.modifier_ids) {
				picked[id] = true;
			}
			modifiers[rating.form_competency_id] = picked;
		}
		const narratives: Record<number, string> = {};
		for (const narrative of view.form.narratives) {
			narratives[narrative.form_narrative_id] = '';
		}
		for (const entry of view.content.narratives) {
			narratives[entry.form_narrative_id] = entry.text;
		}
		this.values = values;
		this.notObserved = notObserved;
		this.modifiers = modifiers;
		this.narratives = narratives;
		this.revision = view.revision;
		// The page has reloaded the winning copy, so a refusal of the
		// revision left behind is answered and may be reported afresh.
		this.#reported_stale = false;
		return replaced;
	}

	/**
	 * Keeps a refused buffer for the author to read and copy. An empty
	 * buffer is nothing to show and is not kept.
	 */
	keepRefused(buffer: RefusedBuffer): void {
		if (buffer.narratives.length === 0 && buffer.ratings.length === 0) {
			return;
		}
		this.refused = [buffer, ...this.refused];
	}

	/**
	 * Drops one refused buffer the author has acknowledged, the only way it
	 * leaves the page besides navigation, logout, or a draft identity
	 * change. It deliberately leaves `saveState` alone: a failed save is a
	 * fact about the working copy, and discarding refused text must not
	 * disarm the guard that keeps a workflow act from submitting it — that
	 * guard is lifted only when the last buffer goes, and by the writer's
	 * own successful save.
	 */
	discardRefused(index: number): void {
		this.refused = this.refused.filter((_, at) => at !== index);
		this.#reported_stale = false;
	}

	/**
	 * The refusal's reload landed: the page's working copy is the winner's,
	 * so whatever the buffers still hold is the writer's to copy or
	 * discard.
	 */
	markReloaded(): void {
		this.#reloadFailed = false;
	}

	/**
	 * The refusal's reload did not land: the page cannot act on a copy it
	 * has not seen, whatever the buffers hold.
	 */
	markReloadFailed(): void {
		this.#reloadFailed = true;
	}

	/**
	 * Whether the working copy now carries everything one refused buffer
	 * held, field for field. A save that carries a buffer's text is the
	 * writer putting it back; a save that does not leaves that buffer
	 * unresolved.
	 */
	#carries(buffer: RefusedBuffer): boolean {
		for (const narrative of buffer.narratives) {
			if ((this.narratives[narrative.form_narrative_id] ?? '') !== narrative.text) {
				return false;
			}
		}
		for (const rating of buffer.ratings) {
			const id = rating.form_competency_id;
			const marked = this.notObserved[id] ?? false;
			const value = marked ? null : (this.values[id] ?? null);
			const picked = Object.entries(this.modifiers[id] ?? {})
				.filter(([, on]) => on)
				.map(([key]) => Number(key))
				.sort((left, right) => left - right);
			const expected = [...rating.modifier_ids].sort((left, right) => left - right);
			if (marked !== rating.not_observed || value !== rating.value) {
				return false;
			}
			if (!sameIds(picked, expected)) {
				return false;
			}
		}
		return true;
	}

	/**
	 * The writer's own save landed. Buffers whose text that save carried
	 * are resolved — the text is in the draft now — while a buffer whose
	 * text it did not carry stays unresolved, so an act still waits for it.
	 * The buffers themselves stay readable until the writer discards them.
	 */
	#resolveCarried(): void {
		this.#reloadFailed = false;
		this.refused = this.refused.map((buffer) =>
			!buffer.resolved && this.#carries(buffer) ? { ...buffer, resolved: true } : buffer
		);
	}

	/**
	 * Ends this controller's work: the refused text is dropped, a late
	 * response from this draft is ignored, and an ordinary edit still
	 * waiting on the debounce is sent rather than discarded. Called on
	 * navigation (including client-side navigation that reuses the route)
	 * and on logout, because the route component is destroyed either way.
	 *
	 * The two are deliberately different. Refused recovery text is a
	 * concurrency answer and lives in this controller, so navigation drops
	 * it; an unsaved edit is the writer's work and navigation must not
	 * silently lose it. Nothing here resubmits text the contract refused:
	 * a refusal the page has not answered yet is skipped, and the buffer is
	 * never part of what a save carries.
	 */
	destroy(): void {
		const draft_id = this.#view()?.id ?? null;
		const pending = (this.#timer !== null || this.#dirty) && !this.#reported_stale;
		this.#destroyed = true;
		if (this.#timer !== null) {
			clearTimeout(this.#timer);
			this.#timer = null;
		}
		this.#dirty = false;
		this.#reported_stale = false;
		this.refused = [];
		if (pending && draft_id !== null) {
			void this.#saveOnTeardown(draft_id);
		}
	}

	/**
	 * One last save for an edit that was still queued when the route went
	 * away. It waits for any in-flight save first, so the revision it sends
	 * is the one the server actually reached, and it reports nothing: the
	 * page that would have shown the outcome is gone.
	 */
	async #saveOnTeardown(draft_id: number): Promise<void> {
		const inflight = this.#inFlight;
		if (inflight !== null) {
			await inflight.catch(() => {});
		}
		if (this.#view()?.id !== draft_id) {
			return;
		}
		try {
			await saveDraftContent(draft_id, this.revision, this.#buildContent());
		} catch {
			// The page is gone; the next visit surfaces the state.
		}
	}

	// --------------------------------------------------------- the content

	/** The content a save would send right now. */
	#buildContent(): DraftContent {
		const view = this.#view();
		if (view === null) {
			return { ratings: [], narratives: [] };
		}
		const ratings: DraftContent['ratings'] = [];
		for (const competency of view.form.competencies) {
			const id = competency.form_competency_id;
			const marked = this.notObserved[id] ?? false;
			const value = marked ? null : (this.values[id] ?? null);
			const picked = Object.entries(this.modifiers[id] ?? {})
				.filter(([, on]) => on)
				.map(([modifierId]) => Number(modifierId));
			if (value !== null || marked || picked.length > 0) {
				ratings.push({
					form_competency_id: id,
					value,
					not_observed: marked,
					modifier_ids: picked
				});
			}
		}
		const texts: DraftContent['narratives'] = [];
		for (const narrative of view.form.narratives) {
			const id = narrative.form_narrative_id;
			const text = this.narratives[id] ?? '';
			if (text !== '') {
				texts.push({ form_narrative_id: id, text });
			}
		}
		return { ratings, narratives: texts };
	}

	// ----------------------------------------------------------- internals

	/** The chain itself. Never reports twice; `#report` is the one exit. */
	async #saveChain(run: SaveRun): Promise<void> {
		// A save that lands resolves the recovery guard only when no refusal
		// arrived while it was in flight: its content is what the writer
		// meant to save, and the server now holds it.
		const refusalsAtStart = this.#refusals;
		try {
			// One attempt per content state: an edit made while a request
			// is pending marks the chain dirty and re-runs the save, so
			// nothing typed during an await is dropped, and an attempt the
			// server refused is never repeated against the same revision.
			for (;;) {
				this.#dirty = false;
				if (this.#stale(run)) {
					return;
				}
				this.saveState = 'saving';
				// The snapshot is taken at request time: it is what this
				// save carries, and the loop below picks up any edit typed
				// while the request was pending.
				run.refused = this.snapshot();
				let saved;
				try {
					saved = await saveDraftContent(
						run.draft_id,
						run.refused.revision,
						this.#buildContent()
					);
				} catch (err) {
					if (this.#stale(run)) {
						return;
					}
					if (err instanceof ApiError && err.code === 'stale_save') {
						if (run.refused.revision !== this.revision) {
							// The page's reload landed while this attempt
							// was in flight; the refusal belongs to a
							// revision already left behind, and a retry
							// now carries the writer's newest text against
							// the winner's revision.
							this.saveState = 'idle';
							continue;
						}
						// Another contributor saved first: their copy wins
						// and is what the page will present. Report the
						// text this save carried so the page can reload the
						// winner and then keep the divergent parts of that
						// text readable (#34). One report per refusal: any
						// later attempt is refused by the same stale
						// revision and carries nothing new.
						this.saveState = 'idle';
						if (!this.#reported_stale) {
							this.#reported_stale = true;
							// Recorded before the page is told, so a
							// workflow act that starts while the refusal's
							// reload is in flight already waits.
							this.#refusals += 1;
							this.#settled = {
								status: 'stale',
								refused: run.refused
							};
						}
						return;
					}
					this.saveState = 'failed';
					this.#settled = {
						status: 'failed',
						message:
							err instanceof ApiError ? err.message : 'the server could not be reached'
					};
					return;
				}
				if (this.#stale(run)) {
					return;
				}
				this.revision = saved.revision;
				this.saveState = 'saved';
				if (this.#refusals === refusalsAtStart) {
					this.#resolveCarried();
				}
				await this.refreshMeta(run.draft_id);
				if (!this.#dirty || this.#destroyed || this.#stale(run)) {
					break;
				}
			}
		} catch {
			// Only the chain's own bookkeeping can fail here: the server
			// call's errors are handled at the attempt.
			if (!this.#stale(run)) {
				this.saveState = 'failed';
				this.#settled = { status: 'failed', message: 'the save could not be completed' };
			}
		}
	}

	/** The result this chain settled as, reported once when it releases. */
	#settled: SaveResult = { status: 'saved' };

	/** Whether this run's result still belongs to the draft on screen. */
	#stale(run: SaveRun): boolean {
		if (this.#destroyed) {
			return true;
		}
		const view = this.#view();
		return view === null || view.id !== run.draft_id;
	}

	/**
	 * The editable state right now, durable against a reload. The page
	 * needs this at catch time as well as inside the chain: text typed
	 * while a refused request was in flight exists only here.
	 */
	snapshot(): EditorSnapshot {
		const view = this.#view();
		const values = new Map<number, number | null>();
		const notObserved = new Map<number, boolean>();
		const modifiers = new Map<number, Set<number>>();
		const narratives = new Map<number, string>();
		for (const [key, value] of Object.entries(this.values)) {
			values.set(Number(key), value);
		}
		for (const [key, value] of Object.entries(this.notObserved)) {
			notObserved.set(Number(key), value);
		}
		for (const [key, picked] of Object.entries(this.modifiers)) {
			modifiers.set(
				Number(key),
				new Set(
					Object.entries(picked ?? {})
						.filter(([, on]) => on)
						.map(([id]) => Number(id))
				)
			);
		}
		for (const [key, text] of Object.entries(this.narratives)) {
			narratives.set(Number(key), text);
		}
		const shape: FormShape = {
			narrative_labels: new Map(),
			competency_names: new Map(),
			modifier_codes: new Map()
		};
		if (view !== null) {
			for (const narrative of view.form.narratives) {
				shape.narrative_labels.set(narrative.form_narrative_id, narrative.prompt);
			}
			for (const competency of view.form.competencies) {
				shape.competency_names.set(competency.form_competency_id, competency.name);
			}
			for (const modifier of view.form.modifiers) {
				shape.modifier_codes.set(modifier.rating_modifier_id, modifier.code);
			}
		}
		return { revision: this.revision, values, notObserved, modifiers, narratives, shape };
	}

	/** Attribution and workflow state, refreshed without clobbering edits. */
	async refreshMeta(draft_id: number): Promise<void> {
		// The view this refresh is about: a reload that lands while the
		// request is in flight replaces the object, and its newer status
		// must not be overwritten by this older answer.
		const view = this.#view();
		if (view === null) {
			return;
		}
		try {
			const fetched = await getDraft(draft_id);
			if (this.#stale({ draft_id, refused: null }) || this.#view() !== view) {
				return;
			}
			view.status = fetched.status;
			view.owner_user_id = fetched.owner_user_id;
			view.owner_display_name = fetched.owner_display_name;
			view.events = fetched.events;
			view.snapshots = fetched.snapshots;
			view.eligible_recipients = fetched.eligible_recipients;
		} catch {
			// The next save or reload surfaces the problem.
		}
	}
}

/** Draft, changes-requested, and returned states edit and resubmit. */
export function openForEditing(view: DraftView): boolean {
	return (
		view.status === 'draft' ||
		view.status === 'changes_requested' ||
		view.status === 'returned'
	);
}

/**
 * The parts of a refused save that differ from the reloaded winning copy.
 * Text the winner already carries is not shown: the exact divergent text
 * is the point (#34), and nothing here merges the two.
 */
export function divergentBuffer(
	refused: EditorSnapshot,
	latest: EditorSnapshot,
	winner: DraftView | null
): RefusedBuffer {
	// With no reloaded copy to compare against (a reload that failed), the
	// whole buffer is recoverable text rather than divergent text: the
	// refusal kept it, and showing nothing would discard it silently.
	const winning_narratives = new Map<number, string>();
	if (winner !== null) {
		for (const narrative of winner.form.narratives) {
			winning_narratives.set(narrative.form_narrative_id, '');
		}
		for (const entry of winner.content.narratives) {
			winning_narratives.set(entry.form_narrative_id, entry.text);
		}
	}
	const narratives: RefusedNarrative[] = [];
	const ids = new Set([...refused.narratives.keys(), ...latest.narratives.keys()]);
	for (const id of ids) {
		const sent = refused.narratives.get(id) ?? '';
		const current = latest.narratives.get(id) ?? sent;
		// The newest text the writer had, and whether it ever reached the
		// server: a save refused at request time never carried an edit made
		// after it was sent, so that text is newer than the refusal.
		const text = current;
		if (text === '' || text === (winning_narratives.get(id) ?? '')) {
			continue;
		}
		narratives.push({
			form_narrative_id: id,
			label:
				latest.shape.narrative_labels.get(id) ??
				refused.shape.narrative_labels.get(id) ??
				`Narrative ${id}`,
			text,
			typed_while_pending: current !== sent
		});
	}
	narratives.sort((left, right) => left.form_narrative_id - right.form_narrative_id);

	const winner_ratings = new Map<
		number,
		{ value: number | null; not_observed: boolean; modifier_ids: number[] }
	>();
	if (winner !== null) {
		for (const rating of winner.content.ratings) {
			winner_ratings.set(rating.form_competency_id, {
				value: rating.value,
				not_observed: rating.not_observed,
				modifier_ids: rating.modifier_ids
			});
		}
	}
	// The competencies to consider: the winner's form, and — when there is
	// no winner to compare against, because the reload failed — the ones
	// the refused save itself named. A rating the writer never touched is
	// not a difference, and a competency the winner has no stored row for
	// reads as the form's default state, so an untouched competency is
	// never shown as divergent (issue #34).
	const competency_ids = new Set<number>();
	for (const competency of winner?.form.competencies ?? []) {
		competency_ids.add(competency.form_competency_id);
	}
	for (const id of refused.shape.competency_names.keys()) {
		competency_ids.add(id);
	}
	for (const id of latest.shape.competency_names.keys()) {
		competency_ids.add(id);
	}
	const ratings: RefusedRating[] = [];
	for (const id of [...competency_ids].sort((left, right) => left - right)) {
		const state = (snapshot: EditorSnapshot) => {
			const not_observed = snapshot.notObserved.get(id) ?? false;
			return {
				not_observed,
				value: not_observed ? null : (snapshot.values.get(id) ?? null),
				picked: [...(snapshot.modifiers.get(id) ?? new Set<number>())].sort(
					(left, right) => left - right
				)
			};
		};
		const sent = state(refused);
		const current = state(latest);
		// No stored row is the form's default, not a divergence.
		const winning = winner_ratings.get(id) ?? {
			value: null,
			not_observed: false,
			modifier_ids: []
		};
		const same =
			winning.not_observed === current.not_observed &&
			winning.value === current.value &&
			sameIds(winning.modifier_ids, current.picked);
		if (same) {
			continue;
		}
		ratings.push({
			form_competency_id: id,
			name:
				winner?.form.competencies.find(
					(candidate) => candidate.form_competency_id === id
				)?.name ??
				latest.shape.competency_names.get(id) ??
				refused.shape.competency_names.get(id) ??
				`Competency ${id}`,
			not_observed: current.not_observed,
			value: current.value,
			modifier_codes: current.picked.map(
				(candidate) =>
					latest.shape.modifier_codes.get(candidate) ??
					refused.shape.modifier_codes.get(candidate) ??
					String(candidate)
			),
			modifier_ids: current.picked,
			typed_while_pending:
				sent.not_observed !== current.not_observed ||
				sent.value !== current.value ||
				!sameIds(sent.picked, current.picked)
		});
	}
	return { narratives, ratings, resolved: false };
}

function sameIds(left: number[], right: number[]): boolean {
	if (left.length !== right.length) {
		return false;
	}
	const sorted = [...left].sort((first, second) => first - second);
	return sorted.every((candidate, index) => candidate === right[index]);
}
