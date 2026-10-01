import { afterEach, describe, expect, it, vi } from 'vitest'

/**
 * `debug.ts` 的开关判定。
 *
 * 这块逻辑琐碎但重要：它决定了「远程页面上能不能看到调试输出」。
 * 而且 `DEBUG` 是在**模块加载时**算一次的快照，所以每个用例都要重置模块再导入 ——
 * 否则第二个用例会拿到第一个用例的判定结果，测试看起来过、其实没测到东西。
 */
async function load(): Promise<typeof import('../debug')> {
  vi.resetModules()
  return await import('../debug')
}

/** 造一个最小的 localStorage 替身。 */
function fakeStorage(initial: Record<string, string> = {}): Storage {
  const map = new Map(Object.entries(initial))
  return {
    getItem: (k: string) => map.get(k) ?? null,
    setItem: (k: string, v: string) => void map.set(k, v),
    removeItem: (k: string) => void map.delete(k),
    clear: () => map.clear(),
    key: (i: number) => [...map.keys()][i] ?? null,
    get length() {
      return map.size
    },
  } as Storage
}

function fakeLocation(search: string, hash: string): Location {
  return { search, hash } as Location
}

afterEach(() => {
  vi.unstubAllGlobals()
  vi.restoreAllMocks()
})

describe('dEBUG 开关判定', () => {
  it('没有任何开关时默认关闭（正常使用不该刷屏）', async () => {
    const { DEBUG } = await load()
    expect(DEBUG).toBe(false)
  })

  it('localStorage 的常见真值写法都算打开', async () => {
    for (const raw of ['1', 'true', 'TRUE', 'on', 'yes', ' 1 ']) {
      vi.stubGlobal('localStorage', fakeStorage({ 'clipbeam-debug': raw }))
      const { DEBUG } = await load()
      expect(DEBUG, `localStorage=${JSON.stringify(raw)}`).toBe(true)
    }
  })

  it('localStorage 的假值算关闭', async () => {
    for (const raw of ['0', 'false', 'off', 'no', '']) {
      vi.stubGlobal('localStorage', fakeStorage({ 'clipbeam-debug': raw }))
      const { DEBUG } = await load()
      expect(DEBUG, `localStorage=${JSON.stringify(raw)}`).toBe(false)
    }
  })

  it('uRL 的 #debug / ?debug=1 也能打开', async () => {
    for (const [search, hash] of [['', '#debug'], ['', '#debug=1'], ['?debug=1', ''], ['?a=1&debug=true', '']]) {
      vi.stubGlobal('location', fakeLocation(search, hash))
      const { DEBUG } = await load()
      expect(DEBUG, `${search}${hash}`).toBe(true)
    }
  })

  it('#debugging 之类的前缀巧合不算打开', async () => {
    for (const hash of ['#debugging', '#mydebug', '#debugx']) {
      vi.stubGlobal('location', fakeLocation('', hash))
      const { DEBUG } = await load()
      expect(DEBUG, hash).toBe(false)
    }
  })

  it('localStorage 优先于 URL —— 可以用它显式压掉 #debug', async () => {
    vi.stubGlobal('location', fakeLocation('', '#debug'))
    vi.stubGlobal('localStorage', fakeStorage({ 'clipbeam-debug': '0' }))
    const { DEBUG } = await load()
    expect(DEBUG).toBe(false)
  })

  it('localStorage 抛错（隐私模式）时退回 URL 判定，而不是把页面搞崩', async () => {
    vi.stubGlobal('location', fakeLocation('', '#debug'))
    vi.stubGlobal('localStorage', {
      getItem: () => {
        throw new Error('SecurityError: 存储被禁用')
      },
    } as unknown as Storage)
    const { DEBUG } = await load()
    expect(DEBUG).toBe(true)
  })
})

describe('debug() 的输出行为', () => {
  it('关闭时是空操作', async () => {
    const spy = vi.spyOn(console, 'debug').mockImplementation(() => {})
    const { debug, DEBUG } = await load()
    expect(DEBUG).toBe(false)
    debug('transfer', '不该出现')
    expect(spy).not.toHaveBeenCalled()
  })

  it('打开时带可 grep 的前缀', async () => {
    vi.stubGlobal('localStorage', fakeStorage({ 'clipbeam-debug': '1' }))
    const spy = vi.spyOn(console, 'debug').mockImplementation(() => {})
    const { debug, DEBUG } = await load()
    expect(DEBUG).toBe(true)
    debug('transfer', '收到分片', 7)
    expect(spy).toHaveBeenCalledWith('[clipbeam:transfer]', '收到分片', 7)
  })
})
