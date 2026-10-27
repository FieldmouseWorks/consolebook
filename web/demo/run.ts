// `npm run demo`: a throwaway installation full of invented records, on
// http://127.0.0.1:7770 until Ctrl-C. Build the interface and the server
// first (`npm run build`, then `cargo build` from the repository root).
// The data directory is scratch and is deleted on exit.

import { startServer } from '../e2e/server.ts';
import { lifecycle } from './lifecycle.ts';
import { DEMO_PASSWORD, PEOPLE, seedDemo } from './seed.ts';

// Ctrl-C is how the demo is meant to stop, so it exits cleanly.
const demo = lifecycle({ signalExitCode: 0 });

await demo.run(async () => {
	const port = demoPort(process.env.CONSOLEBOOK_DEMO_PORT ?? '7770');
	const server = await demo.own(startServer({ bind: `127.0.0.1:${port}` }), (s) => s.stop());
	await seedDemo(server.url, await server.setupCode());

	console.log(`\nConsolebook demo running at ${server.url}\n`);
	console.log(`Every account's password is ${DEMO_PASSWORD}:`);
	for (const { username, display_name, role } of Object.values(PEOPLE)) {
		console.log(`  ${username.padEnd(16)} ${display_name} (${role})`);
	}
	console.log('\nPress Ctrl-C to stop; the demo data is deleted.');
});

/** The demo listens on loopback only; the port is the one choice. */
function demoPort(value: string): number {
	const port = /^\d{1,5}$/.test(value) ? Number(value) : NaN;
	if (!(port >= 0 && port <= 65_535)) {
		throw new Error(`CONSOLEBOOK_DEMO_PORT must be a port number (0 for any free port), not ${value}`);
	}
	return port;
}
