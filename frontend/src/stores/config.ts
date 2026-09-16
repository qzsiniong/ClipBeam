import { invoke } from '@tauri-apps/api/core'
import { defineStore } from 'pinia'
import { ref } from 'vue'

export interface Config {
  send_hotkey: string
  recv_hotkey: string
  stop_hotkey: string
  key_delay_ms: number
  settle_ms: number
  receive_timeout_s: number
  max_text_kb: number
  compress: boolean
}

export const useConfigStore = defineStore('config', () => {
  const config = ref<Config | null>(null)
  const loading = ref(false)
  const error = ref<string | null>(null)

  async function load() {
    loading.value = true
    error.value = null
    try {
      config.value = await invoke<Config>('get_config')
    }
    catch (e) {
      error.value = String(e)
    }
    finally {
      loading.value = false
    }
  }

  async function save(cfg: Config) {
    error.value = null
    try {
      await invoke('save_config', { config: cfg })
      config.value = { ...cfg }
    }
    catch (e) {
      error.value = String(e)
      throw e
    }
  }

  return { config, loading, error, load, save }
})
