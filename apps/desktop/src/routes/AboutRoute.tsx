import { useEffect, useState } from "react";

import type { AppInfo } from "../bindings/AppInfo";
import type { OperationDescriptor } from "../bindings/OperationDescriptor";
import { appInfo, listOperations } from "../ipc";
import { tOr } from "../messages";

export function AboutRoute() {
  const [info, setInfo] = useState<AppInfo | null>(null);
  const [operations, setOperations] = useState<OperationDescriptor[]>([]);

  useEffect(() => {
    void appInfo()
      .then(setInfo)
      .catch(() => setInfo(null));
    // Read the live registry rather than hardcoding a list: a hardcoded one went
    // stale and told users the app could only run a self-test while five engines
    // were shipping.
    void listOperations()
      .then(setOperations)
      .catch(() => setOperations([]));
  }, []);

  const stable = operations.filter((op) => op.stability === "stable");
  const beta = operations.filter((op) => op.stability === "beta");

  return (
    <div className="stack">
      <section className="card">
        <h2>Privacy</h2>
        <p>
          Every file you convert is processed on this computer. LocalConvert has
          no account, no API key, no upload and no remote conversion service. It
          works with the network cable unplugged.
        </p>
        <ul className="ticks">
          <li>Your original files are opened read-only and never modified.</li>
          <li>Results are verified before they are saved to your folder.</li>
          <li>Nothing that fails verification is kept.</li>
          <li>No telemetry is collected in this release.</li>
        </ul>
      </section>

      <section className="card">
        <h2>What this build can do</h2>
        {operations.length === 0 ? (
          <p className="muted">Reading the list of available tools…</p>
        ) : (
          <>
            <ul className="ticks">
              {stable.map((op) => (
                <li key={op.id}>{tOr(`${op.labelKey}`, op.id)}</li>
              ))}
            </ul>
            {beta.length > 0 && (
              <p className="muted">
                In beta:{" "}
                {beta.map((op) => tOr(`${op.labelKey}`, op.id)).join(", ")} —
                these work but have had less cross-platform testing.
              </p>
            )}
          </>
        )}
        <p className="muted">
          Not included, because each needs a codec or renderer this build does
          not bundle: HEIC, AVIF and lossy WebP; turning PDF pages into images.
          These are declined by name rather than half-done. See{" "}
          <code>docs/CONVERSION_MATRIX.md</code> for the tested status of every
          route.
        </p>
      </section>

      <section className="card">
        <h2>Build</h2>
        <dl className="facts">
          <div>
            <dt>Version</dt>
            <dd>{info?.version ?? "…"}</dd>
          </div>
          <div>
            <dt>Platform</dt>
            <dd>{info ? `${info.platform} · ${info.arch}` : "…"}</dd>
          </div>
          <div>
            <dt>Tools</dt>
            <dd>{operations.length}</dd>
          </div>
          <div>
            <dt>Network</dt>
            <dd>{info?.offlineOnly ? "Offline only" : "unknown"}</dd>
          </div>
        </dl>
      </section>
    </div>
  );
}
