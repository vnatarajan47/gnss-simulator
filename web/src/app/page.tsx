import Workbench from "@/components/Workbench";

/**
 * The whole app is one interactive view. It stays a server component so the
 * page shell renders without waiting on WASM; `Workbench` is the client
 * boundary where the map and the WASM module take over.
 */
export default function Page() {
  return <Workbench />;
}
