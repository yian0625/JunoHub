import type { NextConfig } from "next";

const nextConfig: NextConfig = {
  output: "standalone",
  devIndicators: false,
  allowedDevOrigins: ["127.0.0.1"],
  async rewrites() {
    const apiUrl = process.env.JUNO_HUB_API_URL || "http://127.0.0.1:9003"
    return [
      {
        source: "/hub_admin/:path*",
        destination: `${apiUrl}/hub_admin/:path*`,
      },
      {
        source: "/auth/:path*",
        destination: `${apiUrl}/auth/:path*`,
      },
      {
        source: "/console-api/:path*",
        destination: `${apiUrl}/console/:path*`,
      },
      {
        source: "/v1/:path*",
        destination: `${apiUrl}/v1/:path*`,
      },
    ]
  },
};

export default nextConfig;
