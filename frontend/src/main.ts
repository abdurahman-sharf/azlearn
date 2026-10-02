import { createApp } from 'vue'
import { createPinia } from 'pinia'
import App from './App.vue'
import router from './router'
import './assets/main.css'
import './assets/fonts.css'
import { useAuthStore } from './stores/auth'
import { platformEnabled } from './lib/supabase'

const app = createApp(App)
app.use(createPinia())
app.use(router)
app.mount('#app')

// Restore the platform session in the background; public routes never wait for it.
if (platformEnabled) void useAuthStore().init()
