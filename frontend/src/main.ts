import { createApp } from 'vue'
import { createPinia } from 'pinia'
import App from './App.vue'
import router from './router'
import './assets/main.css'
import './assets/fonts.css'
import { useAuthStore } from './stores/auth'
import { useBrandingStore } from './stores/branding'
import { platformEnabled } from './lib/platformApi'

// The azlearn palette applies to the platform (web/Docker); desktop/CF builds keep the original theme.
if (platformEnabled) document.documentElement.setAttribute('data-brand', 'azlearn')

const app = createApp(App)
app.use(createPinia())
app.use(router)
app.mount('#app')

// Restore the platform session in the background; public routes never wait for it.
if (platformEnabled) {
  void useAuthStore().init()
  void useBrandingStore().load()
}
