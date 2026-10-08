import { defineConfig } from 'vite';
import solid from 'vite-plugin-solid';
import { viteSingleFile } from 'vite-plugin-singlefile';

const remuda = {
  target: `http://127.0.0.1:${process.env.REMUDA_PORT ?? 7429}`,
  changeOrigin: true,
};

export default defineConfig({
  plugins: [solid(), viteSingleFile()],
  build: {
    outDir: '../src',
    emptyOutDir: false,
    modulePreload: { polyfill: false },
    rollupOptions: { input: 'serve.html' },
  },
  server: {
    proxy: {
      '/api': {
        ...remuda,
        configure: (proxy) => proxy.on('proxyReq', (req) => req.removeHeader('origin')),
      },
      '/favicon.svg': remuda,
    },
  },
});
