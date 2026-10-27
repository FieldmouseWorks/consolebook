// A demo command owns a server, perhaps a browser, and the server's scratch
// directory. However the command ends (done, failed, or SIGINT/SIGTERM at
// any moment, startup included), each is released once before the process
// exits, and one release failing doesn't stop the others. Once ending has
// begun the command goes no further: anything it acquires after that is
// released by the same ending, and it never gets the resource back.

type Release = () => Promise<void>;

export function lifecycle({
	signalExitCode,
	exit = (code: number) => process.exit(code),
	signals = true
}: {
	signalExitCode: number;
	/** Tests replace these; a command keeps the defaults. */
	exit?: (code: number) => void;
	signals?: boolean;
}) {
	const releases: Release[] = [];
	let ending: Promise<void> | undefined;

	/** Releases everything owned, newest first, until nothing is left, then exits. */
	function end(exitCode: number): Promise<void> {
		ending ??= (async () => {
			let failed = false;
			// A release can await an acquisition that lets the command take
			// another step, so drain until no release is left, not a snapshot.
			for (let release = releases.pop(); release; release = releases.pop()) {
				try {
					await release();
				} catch (error) {
					failed = true;
					console.error(error);
				}
			}
			exit(failed && exitCode === 0 ? 1 : exitCode);
		})();
		return ending;
	}

	if (signals) {
		for (const signal of ['SIGINT', 'SIGTERM'] as const) {
			process.on(signal, () => void end(signalExitCode));
		}
	}

	return {
		end,
		/**
		 * Owns a resource from the moment its acquisition starts: a signal
		 * mid-startup waits for it to settle, then releases it. After ending
		 * has begun the command never receives it, so it can't step further.
		 */
		own<T>(acquiring: Promise<T>, release: (resource: T) => Promise<void>): Promise<T> {
			releases.push(async () => {
				const resource = await acquiring.then(
					(value) => ({ value }),
					() => undefined
				);
				if (resource) await release(resource.value);
			});
			return acquiring.then((value) => (ending ? new Promise<T>(() => {}) : value));
		},
		/** Runs the command; a failure is reported and ends it. */
		async run(work: () => Promise<void>): Promise<void> {
			try {
				await work();
			} catch (error) {
				console.error(error);
				await end(1);
			}
		}
	};
}
