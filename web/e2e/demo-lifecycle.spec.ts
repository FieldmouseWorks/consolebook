// The demo commands' teardown, driven with promises the test settles by
// hand, so each race happens in a known order.

import { lifecycle } from '../demo/lifecycle.ts';
import { expect, test } from '@playwright/test';

function deferred<T>() {
	let resolve!: (value: T) => void;
	const promise = new Promise<T>((settle) => (resolve = settle));
	return { promise, resolve };
}

function harness() {
	const exited = deferred<number>();
	let exits = 0;
	const released: string[] = [];
	const demo = lifecycle({
		signalExitCode: 130,
		signals: false,
		exit: (code) => {
			exits += 1;
			exited.resolve(code);
		}
	});
	const release = async (name: string) => {
		released.push(name);
	};
	return { demo, exited: exited.promise, released, release, exits: () => exits };
}

test('ending during startup halts the command before it acquires more', async () => {
	const { demo, exited, released, release } = harness();
	const server = deferred<string>();
	let launched = false;
	void demo.run(async () => {
		await demo.own(server.promise, release);
		launched = true;
		await demo.own(Promise.resolve('browser'), release);
	});
	void demo.end(130);
	server.resolve('server');
	expect(await exited).toBe(130);
	expect(released).toEqual(['server']);
	expect(launched).toBe(false);
});

test('ending drains an acquisition still settling, newest first', async () => {
	const { demo, exited, released, release } = harness();
	const browser = deferred<string>();
	let captured = false;
	const started = deferred<void>();
	void demo.run(async () => {
		await demo.own(Promise.resolve('server'), release);
		const pending = demo.own(browser.promise, release);
		started.resolve();
		await pending;
		captured = true;
	});
	await started.promise;
	void demo.end(130);
	browser.resolve('browser');
	expect(await exited).toBe(130);
	expect(released).toEqual(['browser', 'server']);
	expect(captured).toBe(false);
});

test('a failed release neither stops the others nor passes for success', async () => {
	const { demo, exited, released, release } = harness();
	await demo.own(Promise.resolve('server'), release);
	await demo.own(Promise.resolve('browser'), async () => {
		throw new Error('browser would not close');
	});
	void demo.end(0);
	expect(await exited).toBe(1);
	expect(released).toEqual(['server']);
});

test('concurrent endings share one teardown and one exit', async () => {
	const { demo, exited, released, release, exits } = harness();
	await demo.own(Promise.resolve('server'), release);
	const first = demo.end(130);
	const second = demo.end(1);
	expect(second).toBe(first);
	await first;
	expect(await exited).toBe(130);
	expect(released).toEqual(['server']);
	expect(exits()).toBe(1);
});
