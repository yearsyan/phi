import react from '@vitejs/plugin-react';
import { defineConfig } from 'vite';

const daemonProxyTarget =
  process.env.PHI_WEB_DAEMON_PROXY_TARGET?.trim() || 'http://127.0.0.1:8787';

export default defineConfig({
  plugins: [react()],
  server: {
    proxy: {
      '/v1': {
        target: daemonProxyTarget,
        changeOrigin: true,
        ws: true,
      },
    },
  },
});
