import { getCurrentWindow } from "@tauri-apps/api/window";
import Overlay from "./pages/Overlay";
import Settings from "./pages/Settings";

const label = getCurrentWindow().label;

export default function App() {
  if (label === "overlay") return <Overlay />;
  if (label === "settings") return <Settings />;
  return null;
}
