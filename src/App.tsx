import { getCurrentWindow } from "@tauri-apps/api/window";
import Overlay from "./pages/Overlay";
import Processing from "./pages/Processing";
import Settings from "./pages/Settings";
import Bubble from "./pages/Bubble";
import BubbleMenu from "./pages/BubbleMenu";
import Onboarding from "./pages/Onboarding";

const label = getCurrentWindow().label;

export default function App() {
  if (label === "overlay") return <Overlay />;
  if (label === "processing") return <Processing />;
  if (label === "settings") return <Settings />;
  if (label === "bubble") return <Bubble />;
  if (label === "bubble_menu") return <BubbleMenu />;
  if (label === "onboarding") return <Onboarding />;
  return null;
}
