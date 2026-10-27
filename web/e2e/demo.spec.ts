// `npm run demo` and `npm run screenshots` seed through the public API, so
// the demo breaks whenever the API moves on without it. Seeding here keeps
// it honest.

import { agencyCalendar, seedDemo } from '../demo/seed.ts';
import { expect, test } from './fixtures';

test('the demo seeds a fresh installation', async ({ server, setupCode }) => {
	const seeded = await seedDemo(server.url, setupCode);
	expect(seeded.openDraft).toBeGreaterThan(0);
	expect(seeded.amendedRecord).toBeGreaterThan(0);
});

test('the demo counts back calendar days through daylight-saving changes', () => {
	// 23:30 on 9 November 2025 in Los Angeles, a week after clocks fell back:
	// 24-hour steps from here land on 2 November twice.
	const autumn = agencyCalendar(new Date('2025-11-10T07:30:00Z'));
	expect([0, 6, 7, 8].map(autumn.date)).toEqual([
		'2025-11-09',
		'2025-11-03',
		'2025-11-02',
		'2025-11-01'
	]);
	// Each day starts at local midnight: PST after the change, PDT before.
	expect(autumn.dayStart(7)).toBe(Date.parse('2025-11-02T07:00:00Z') / 1000);
	expect(autumn.dayStart(6)).toBe(Date.parse('2025-11-03T08:00:00Z') / 1000);

	// 00:30 on 9 March 2026, the night after clocks sprang forward: 24-hour
	// steps from here skip 8 March.
	const spring = agencyCalendar(new Date('2026-03-09T07:30:00Z'));
	expect([0, 1, 2].map(spring.date)).toEqual(['2026-03-09', '2026-03-08', '2026-03-07']);
	expect(spring.dayStart(1)).toBe(Date.parse('2026-03-08T08:00:00Z') / 1000);
	expect(spring.dayStart(0)).toBe(Date.parse('2026-03-09T07:00:00Z') / 1000);
});
