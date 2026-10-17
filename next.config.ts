import type { NextConfig } from "next";

const nextConfig: NextConfig = {
  output: "export",
  images: { unoptimized: true },
  devIndicators: false,
  reactStrictMode: false,
  agentRules: false,
};

export default nextConfig;
