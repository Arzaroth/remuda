import { toastState } from '../state';

export function Toast() {
  return (
    <div
      id="toast"
      role="status"
      classList={{ show: toastState().show, error: toastState().error }}
    >
      {toastState().message}
    </div>
  );
}
