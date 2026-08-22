// ============================================================================
// Agent Studio - Main Application
// A single-purpose skills.sh manager: browse, install, and manage skills
// ============================================================================

import { SkillStore } from "./components/SkillStore";
import { ToastContainer } from "./components/ui/ToastContainer";
import "./App.css";

function App() {
  return (
    <div className="flex h-screen overflow-hidden bg-[var(--color-bg-primary)]">
      <div className="flex flex-1 overflow-hidden">
        <SkillStore />
      </div>

      {/* Toast Notifications */}
      <ToastContainer />
    </div>
  );
}

export default App;
