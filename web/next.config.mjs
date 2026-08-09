/** @type {import('next').NextConfig} */
const nextConfig = {
  // wasm-pack emits an ES module that instantiates asynchronously. Next's
  // bundler needs this on to follow the `new URL(..., import.meta.url)` the
  // generated glue uses to locate the .wasm file.
  webpack: (config) => {
    config.experiments = { ...config.experiments, asyncWebAssembly: true };
    return config;
  },
};

export default nextConfig;
