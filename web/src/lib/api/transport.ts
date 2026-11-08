// Shared same-origin request and typed-error boundary.

/** Error body shape the API guarantees for non-2xx responses. */
export interface ApiErrorBody {
	error: string;
	message: string;
	/** Itemized refusal reasons, present on validation refusals. */
	problems?: string[];
}

export class ApiError extends Error {
	readonly status: number;
	readonly code: string;
	readonly problems: string[];

	constructor(status: number, body: ApiErrorBody) {
		super(body.message);
		this.status = status;
		this.code = body.error;
		this.problems = body.problems ?? [];
	}
}

export async function request<T>(path: string, init?: RequestInit): Promise<T> {
	const response = await fetch(path, {
		headers: init?.body ? { 'Content-Type': 'application/json' } : undefined,
		...init
	});
	if (response.status === 204) {
		return undefined as T;
	}
	await requireOk(response);
	return (await response.json()) as T;
}

/** Shared typed refusal handling for JSON calls and archive downloads. */
export async function requireOk(response: Response): Promise<void> {
	if (!response.ok) {
		let body: ApiErrorBody;
		try {
			body = (await response.json()) as ApiErrorBody;
		} catch {
			body = { error: 'unreachable', message: `server returned ${response.status}` };
		}
		throw new ApiError(response.status, body);
	}
}
