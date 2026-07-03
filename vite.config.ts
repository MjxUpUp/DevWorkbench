import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
// 显式 .mjs 扩展名：rolldown bundle vite.config 时对无扩展名 import 解析不到
// scripts/gen-invoked-commands.mjs（只入库了 .mjs + 配套声明，无 .ts 源），导致 vite
// build 加载配置即 UNRESOLVED_IMPORT 失败。配套声明文件用 .d.mts（TS 对 .mjs 的标准
// 类型声明扩展名，bundler 模式下显式 .mjs import 查 .d.mts 而非 .d.ts）。
import { generateInvokedCommandsFile } from './scripts/gen-invoked-commands.mjs'

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
