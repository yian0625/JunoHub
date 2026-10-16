import type { NextConfig } from "next";

const nextConfig: NextConfig = {
  output: "standalone",
  devIndicators: false,
  // Leave headroom so the Rust gateway can reject >32MiB bodies with 413
  // instead of Next truncating a stream while retaining its Content-Length.
  experimental: { proxyClientMaxBodySize: 64 * 1024 * 1024 },
  allowedDevOrigins: ["127.0.0.1"],
  async headers() {
    // 会话令牌存在 localStorage 里，因此把 XSS 的可利用面尽量压小：
    // 禁止外部脚本、禁止被 iframe 嵌套、禁止插件与设备 API。
    // 说明：Next 的注水脚本是内联的，未使用 nonce，所以 script-src 仍需要
    // 'unsafe-inline'；它挡不住现成的内联注入，但能挡住所有外部脚本与
    // 外部连接，仍是有效的一道防线。
    const dev = process.env.NODE_ENV === "development"
    const csp = [
      "default-src 'self'",
      "base-uri 'self'",
      "form-action 'self'",
      "frame-ancestors 'none'",
      "object-src 'none'",
      `script-src 'self' 'unsafe-inline'${dev ? " 'unsafe-eval'" : ""}`,
      "style-src 'self' 'unsafe-inline'",
      "img-src 'self' data: blob: https:",
      "font-src 'self' data:",
      "connect-src 'self'",
      "worker-src 'self' blob:",
      "manifest-src 'self'",
    ].join("; ")

    const securityHeaders = [
      { key: "Content-Security-Policy", value: csp },
      { key: "X-Content-Type-Options", value: "nosniff" },
      { key: "X-Frame-Options", value: "DENY" },
      { key: "Referrer-Policy", value: "strict-origin-when-cross-origin" },
      { key: "Permissions-Policy", value: "camera=(), microphone=(), geolocation=(), payment=()" },
      { key: "Cross-Origin-Opener-Policy", value: "same-origin" },
      // 仅在 HTTPS 下生效；HTTP 直接被浏览器忽略，放在这里是为了让
      // 反代终止 TLS 的部署也能拿到 HSTS。
      { key: "Strict-Transport-Security", value: "max-age=31536000; includeSubDomains" },
    ]

    return [
      { source: "/:path*", headers: securityHeaders },
      ...["/console", "/console/:path*"].map(source => ({
        source,
        headers: [{ key: "Cache-Control", value: "private, no-store, max-age=0, must-revalidate" }],
      })),
    ]
  },
  async rewrites() {
    const apiUrl = process.env.JUNO_HUB_API_URL || "http://127.0.0.1:9003"
    return [
      {
        source: "/hub_admin/:path*",
        destination: `${apiUrl}/hub_admin/:path*`,
      },
      { source: "/auth/login", destination: `${apiUrl}/auth/login` },
      { source: "/auth/register", destination: `${apiUrl}/auth/register` },
      { source: "/auth/register/code", destination: `${apiUrl}/auth/register/code` },
      { source: "/client-connect/redeem", destination: `${apiUrl}/client-connect/redeem` },
      { source: "/media/avatars/:assetId", destination: `${apiUrl}/media/avatars/:assetId` },
      {
        source: "/account-api/:path*",
        destination: `${apiUrl}/account-api/:path*`,
      },
      {
        source: "/console-api/:path*",
        destination: `${apiUrl}/console/:path*`,
      },
      {
        source: "/v1/:path*",
        destination: `${apiUrl}/v1/:path*`,
      },
      {
        source: "/v1beta/:path*",
        destination: `${apiUrl}/v1beta/:path*`,
      },
    ]
  },
};

export default nextConfig;
