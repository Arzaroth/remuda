import { defineConfig } from 'vite';
import solid from 'vite-plugin-solid';
import { viteSingleFile } from 'vite-plugin-singlefile';

export default defineConfig({
  plugins: [solid(), viteSingleFile()],
  build: {
    // serve.rs compiles the page in with include_str!, so it has to be one file.
    outDir: '../src',
    emptyOutDir: false,
    modulePreload: { polyfill: false },
    rollupOptions: { input: 'serve.html' },
  },
  server: {
    proxy: {
      '/api': {
        target: `http://127.0.0.1:${process.env.REMUDA_PORT ?? 7429}`,
        changeOrigin: true,
        configure: (proxy) => proxy.on('proxyReq', (req) => req.removeHeader('origin')),
      },
      '/favicon.svg': `http://127.0.0.1:${process.env.REMUDA_PORT ?? 7429}`,
    },
  },
});
