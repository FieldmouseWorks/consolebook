// A demo command owns a server, perhaps a browser, and the server's scratch
// directory. However the command ends (done, failed, or SIGINT/SIGTERM at
// any moment, startup included), each is released once before the process
// exits, and one release failing doesn't stop the others.

type Release = () => Promise<void>;

export function lifecycle({ signalExitCode }: { signalExitCode: number }) {
	const releases: Release[] = [];
	let ending: Promise<never> | undefined;

	/** Releases everything owned, newest first, then exits. */
	function end(exitCode: number): Promise<never> {
		ending ??= (async () => {
			let failed = false;
			for (const release of [...releases].reverse()) {
				try {
					await release();
				} catch (error) {
					failed = true;
					console.error(error);
				}
			}
			process.exit(failed && exitCode === 0 ? 1 : exitCode);
		})();
		return ending;
	}

	for (const signal of ['SIGINT', 'SIGTERM'] as const) {
		process.on(signal, () => void end(signalExitCode));
	}

	return {
		end,
		/**
		 * Owns a resource from the moment its acquisition starts: a signal
		 * mid-startup waits for it to settle, then releases it.
		 */
		own<T>(acquiring: Promise<T>, release: (resource: T) => Promise<void>): Promise<T> {
			releases.push(async () => {
				const resource = await acquiring.then(
					(value) => ({ value }),
					() => undefined
				);
				if (resource) await release(resource.value);
			});
			return acquiring;
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
