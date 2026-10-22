import {defineConfig} from '@playwright/test';
import {fileURLToPath} from 'node:url';
export default defineConfig({
  testDir:'.',testMatch:['ui.spec.mjs','mcp-apps.spec.mjs'],fullyParallel:true,
  use:{baseURL:'http://127.0.0.1:3101',channel:process.env.PLAYWRIGHT_CHANNEL||'chromium',viewport:{width:1280,height:900}},
  webServer:{command:'node test/serve-ui.mjs',cwd:fileURLToPath(new URL('..',import.meta.url)),port:3101,reuseExistingServer:false},
});
