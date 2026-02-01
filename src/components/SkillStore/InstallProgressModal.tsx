// ============================================================================
// InstallProgressModal - Shows installation progress
// ============================================================================

import { X } from 'lucide-react';
import type { InstallProgressState } from '../../lib/skillsTypes';

interface InstallProgressModalProps {
  progress: InstallProgressState;
  onClose: () => void;
}

export function InstallProgressModal({ progress, onClose }: InstallProgressModalProps) {
  return (
    <div className="install-progress-overlay" onClick={onClose}>
      <div className="install-progress-modal" onClick={e => e.stopPropagation()}>
        <div className="install-progress-header">
          <h3>Installing {progress.skillName}</h3>
          <button className="install-progress-close" onClick={onClose}>
            <X size={18} />
          </button>
        </div>

        <div className="install-progress-content">
          <div className="install-progress-spinner" />
          <p className="install-progress-stage">{progress.stage}</p>
          <p className="install-progress-message">{progress.message}</p>

          {progress.percent !== undefined && (
            <div className="install-progress-bar">
              <div
                className="install-progress-bar-fill"
                style={{ width: `${progress.percent}%` }}
              />
            </div>
          )}

          {progress.error && (
            <div className="install-progress-error">
              <p>{progress.error}</p>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
