import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import { generateInvokedCommandsFile } from './scripts/gen-invoked-commands'

// https://vite.dev/config/
export default defineConfig({
  plugins: [
    react(),
    {
      // P4 平台自审：dev server 启动 + build 开始时，重新生成前端 invoke 集合 manifest
      // （src/generated/invoked-commands.ts，P4 自审的 F 真相源）。失败只 warn 不阻断
      // ——manifest 已入库，临时生成失败回退到上次入库版本，不卡 dev/build。
      name: 'gen-invoked-commands',
      buildStart() {
        try {
          generateInvokedCommandsFile();
        } catch (e) {
          console.warn('[gen-invoked-commands] regenerate failed, using committed manifest:', e);
        }
      },
      configureServer() {
        try {
          generateInvokedCommandsFile();
        } catch (e) {
          console.warn('[gen-invoked-commands] regenerate failed, using committed manifest:', e);
        }
      },
    },
  ],
})
