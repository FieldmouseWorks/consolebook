import adapter from '@sveltejs/adapter-static';
import { sveltekit } from '@sveltejs/kit/vite';
import { defineConfig } from 'vite';

export default defineConfig({
	plugins: [
		sveltekit({
			// Single-page app: every route is client-rendered and unknown paths
			// fall back to index.html, served by the Rust executable.
			adapter: adapter({ fallback: 'index.html' })
		})
	],
	server: {
		// Development convenience only; production serves everything from
		// the Rust executable.
		proxy: {
			'/api': 'http://127.0.0.1:7770'
		}
	}
});
