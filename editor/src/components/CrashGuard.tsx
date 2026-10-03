import { Component, useEffect, useState, type ReactNode } from "react";

/**
 * Last line of defense. A React render error, or the WASM core aborting
 * (a Rust panic or trap leaves the instance unusable), shows this screen
 * instead of a broken editor. Unsaved work is in the autosave and is
 * offered back on reload.
 */
export function CrashGuard({ children }: { children: ReactNode }) {
  const [fatal, setFatal] = useState<string | null>(null);
  useEffect(() => {
    const isCoreAbort = (e: unknown) => e instanceof WebAssembly.RuntimeError;
    const onError = (ev: ErrorEvent) => isCoreAbort(ev.error) && setFatal(String(ev.error?.message ?? ev.message));
    const onRejection = (ev: PromiseRejectionEvent) => isCoreAbort(ev.reason) && setFatal(String(ev.reason?.message ?? ev.reason));
    window.addEventListener("error", onError);
    window.addEventListener("unhandledrejection", onRejection);
    return () => {
      window.removeEventListener("error", onError);
      window.removeEventListener("unhandledrejection", onRejection);
    };
  }, []);
  if (fatal) return <CrashScreen message={`The core stopped unexpectedly (${fatal}).`} />;
  return <Boundary>{children}</Boundary>;
}

class Boundary extends Component<{ children: ReactNode }, { error: Error | null }> {
  state = { error: null as Error | null };
  static getDerivedStateFromError(error: Error) {
    return { error };
  }
  componentDidCatch(error: Error) {
    console.error("Zoetrope UI error", error);
  }
  render() {
    return this.state.error ? <CrashScreen message={this.state.error.message} /> : this.props.children;
  }
}

function CrashScreen({ message }: { message: string }) {
  return (
    <div className="boot crash">
      <h2>Zoetrope hit an unexpected error</h2>
      <p className="crash-message">{message}</p>
      <p>Unsaved work up to the last autosave will be offered when the editor restarts.</p>
      <p className="muted">Details are in the developer console; please include them in a bug report.</p>
      <button onClick={() => location.reload()}>Restart editor</button>
    </div>
  );
}
