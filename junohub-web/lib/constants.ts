/**
 * 与后端 Go 常量同步 (store/model/)
 * 修改时务必同步后端定义
 */

// ===== UserStatus (model/user_info.go) =====
export const UserStatus = {
  ACTIVE: 0,
  BANNED: 1,
  DELETED: 2,
} as const

export const USER_STATUS_MAP: Record<number, { label: string; className: string }> = {
  [UserStatus.ACTIVE]:  { label: "正常", className: "bg-green-500/10 text-green-600 dark:text-green-400" },
  [UserStatus.BANNED]:  { label: "封禁", className: "bg-red-500/10 text-red-600 dark:text-red-400" },
  [UserStatus.DELETED]: { label: "注销", className: "bg-muted text-muted-foreground" },
}

// ===== UserRole (model/user_info.go) =====
export const UserRole = {
  DEFAULT: 0,
  ADMIN: 100,
} as const

// ===== LoginType (model/user_info.go) =====
export const LoginType = {
  ACCOUNT: 1,
  VISITOR: 99,
} as const

export const LOGIN_TYPE_MAP: Record<number, string> = {
  [LoginType.ACCOUNT]: "账号",
  [LoginType.VISITOR]: "游客",
}

// ===== AssistantType (model/assistant.go) =====
export const AssistantType = {
  PRIVATE: 0,
  PUBLISHED: 1,
  SYSTEM: 2,
} as const

export const ASSISTANT_TYPE_MAP: Record<number, { label: string; className: string }> = {
  [AssistantType.PRIVATE]:   { label: "私有", className: "bg-muted text-muted-foreground" },
  [AssistantType.PUBLISHED]: { label: "已发布", className: "bg-green-500/10 text-green-600 dark:text-green-400" },
  [AssistantType.SYSTEM]:    { label: "系统", className: "bg-blue-500/10 text-blue-600 dark:text-blue-400" },
}

// ===== ReviewStatus (model/assistant_review.go) =====
export const ReviewStatus = {
  PENDING: 0,
  APPROVED: 1,
  REJECTED: 2,
} as const

export const REVIEW_STATUS_MAP: Record<number, { label: string; className: string }> = {
  [ReviewStatus.PENDING]:  { label: "待审核", className: "bg-yellow-500/10 text-yellow-600 dark:text-yellow-400" },
  [ReviewStatus.APPROVED]: { label: "已通过", className: "bg-green-500/10 text-green-600 dark:text-green-400" },
  [ReviewStatus.REJECTED]: { label: "已拒绝", className: "bg-red-500/10 text-red-600 dark:text-red-400" },
}
