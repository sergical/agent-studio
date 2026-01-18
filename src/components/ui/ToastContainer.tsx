// ============================================================================
// ToastContainer - Container for managing toast notifications
// ============================================================================

import { AnimatePresence } from 'motion/react';
import { Toast } from './Toast';
import { useAppStore, selectToasts } from '../../store/appStore';

export function ToastContainer() {
  const toasts = useAppStore(selectToasts);
  const removeToast = useAppStore(state => state.removeToast);
  
  if (toasts.length === 0) return null;
  
  return (
    <div className="toast-container">
      <AnimatePresence mode="popLayout">
        {toasts.map((toast) => (
          <Toast key={toast.id} toast={toast} onDismiss={removeToast} />
        ))}
      </AnimatePresence>
    </div>
  );
}
