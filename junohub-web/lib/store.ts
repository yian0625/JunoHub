import { create } from 'zustand'
import { getToken, getUser, setToken, setUser, removeToken, removeUser } from './api'

interface HubUser {
  uid?: string | number
  nickname?: string
  email?: string
  avatar_url?: string
  role?: "admin" | "user"
}

interface AuthState {
  token: string | null
  user: HubUser | null
  isLoggedIn: boolean
  isReady: boolean
  login: (token: string, user: HubUser) => void
  updateUser: (user: Partial<HubUser>) => void
  logout: () => void
  init: () => void
}

export const useAuthStore = create<AuthState>((set) => ({
  token: null,
  user: null,
  isLoggedIn: false,
  isReady: false,

  login: (token: string, user: HubUser) => {
    setToken(token)
    setUser(user)
    set({ token, user, isLoggedIn: true, isReady: true })
  },

  updateUser: (patch: Partial<HubUser>) => {
    set((state) => {
      if (!state.user) return state
      const user = { ...state.user, ...patch }
      setUser(user)
      return { user }
    })
  },

  logout: () => {
    removeToken()
    removeUser()
    set({ token: null, user: null, isLoggedIn: false, isReady: true })
    window.location.replace('/login')
  },

  init: () => {
    const token = getToken()
    const user = getUser<HubUser>()
    set({ token, user: token ? user : null, isLoggedIn: !!token, isReady: true })
  },
}))
