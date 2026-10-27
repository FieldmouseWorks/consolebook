// `npm run demo`: a throwaway installation full of invented records, on
// http://127.0.0.1:7770 until Ctrl-C. Build the interface and the server
// first (`npm run build`, then `cargo build` from the repository root).
// The data directory is scratch and is deleted on exit.

import { startServer } from '../e2e/server.ts';
import { DEMO_PASSWORD, PEOPLE, seedDemo } from './seed.ts';

const bind = process.env.CONSOLEBOOK_DEMO_BIND ?? '127.0.0.1:7770';
const server = await startServer({ bind });
try {
	await seedDemo(server.url, await server.setupCode());
} catch (error) {
	await server.stop();
	throw error;
}

console.log(`\nConsolebook demo running at ${server.url}\n`);
console.log(`Every account's password is ${DEMO_PASSWORD}:`);
for (const { username, display_name, role } of Object.values(PEOPLE)) {
	console.log(`  ${username.padEnd(16)} ${display_name} (${role})`);
}
console.log('\nPress Ctrl-C to stop; the demo data is deleted.');

for (const signal of ['SIGINT', 'SIGTERM'] as const) {
	process.once(signal, () => {
		void server.stop().then(() => process.exit(0));
	});
}
