import {
  createHashHistory,
  createRootRoute,
  createRoute,
  createRouter,
  Link,
  Outlet,
} from "@tanstack/react-router";
import { useEffect } from "react";

import { listJobs, onJobUpdated } from "./ipc";
import { AboutRoute } from "./routes/AboutRoute";
import { ArchiveRoute } from "./routes/ArchiveRoute";
import { ConvertRoute } from "./routes/ConvertRoute";
import { DataRoute } from "./routes/DataRoute";
import { MediaRoute } from "./routes/MediaRoute";
import { PdfRoute } from "./routes/PdfRoute";
import { HomeRoute } from "./routes/HomeRoute";
import { QueueRoute } from "./routes/QueueRoute";
import { activeJobs, useJobStore } from "./store";

function Shell() {
  const jobs = useJobStore((state) => state.jobs);
  const setJobs = useJobStore((state) => state.setJobs);
  const upsertJob = useJobStore((state) => state.upsertJob);
  const running = activeJobs(jobs).length;

  useEffect(() => {
    void listJobs().then(setJobs).catch(() => undefined);
    const unlisten = onJobUpdated(upsertJob);
    return () => {
      void unlisten.then((stop) => stop()).catch(() => undefined);
    };
  }, [setJobs, upsertJob]);

  return (
    <div className="app">
      <header className="app__header">
        <div className="brand">
          <span className="brand__mark" aria-hidden="true">
            ⇄
          </span>
          <h1>LocalConvert</h1>
        </div>
        <nav className="nav" aria-label="Conversion tools">
          <Link to="/" className="navlink" activeProps={{ className: "navlink navlink--active" }}>
            Images
          </Link>
          <Link
            to="/archives"
            className="navlink"
            activeProps={{ className: "navlink navlink--active" }}
          >
            Archives
          </Link>
          <Link
            to="/pdf"
            className="navlink"
            activeProps={{ className: "navlink navlink--active" }}
          >
            PDF
          </Link>
          <Link
            to="/media"
            className="navlink"
            activeProps={{ className: "navlink navlink--active" }}
          >
            Media
          </Link>
          <Link
            to="/data"
            className="navlink"
            activeProps={{ className: "navlink navlink--active" }}
          >
            Data
          </Link>
        </nav>
        <nav className="nav nav--system" aria-label="Application">
          <span className="offline" title="No network access — this app has no HTTP client">
            <span aria-hidden="true">●</span> Offline
          </span>
          <Link
            to="/queue"
            className="navlink"
            activeProps={{ className: "navlink navlink--active" }}
          >
            Queue{running > 0 && <span className="pill">{running}</span>}
          </Link>
          <Link
            to="/diagnostics"
            className="navlink"
            activeProps={{ className: "navlink navlink--active" }}
          >
            Diagnostics
          </Link>
          <Link
            to="/about"
            className="navlink"
            activeProps={{ className: "navlink navlink--active" }}
          >
            About
          </Link>
        </nav>
      </header>
      <main className="app__main">
        <Outlet />
      </main>
    </div>
  );
}

const rootRoute = createRootRoute({ component: Shell });

const routeTree = rootRoute.addChildren([
  createRoute({ getParentRoute: () => rootRoute, path: "/", component: ConvertRoute }),
  createRoute({ getParentRoute: () => rootRoute, path: "/archives", component: ArchiveRoute }),
  createRoute({ getParentRoute: () => rootRoute, path: "/data", component: DataRoute }),
  createRoute({ getParentRoute: () => rootRoute, path: "/pdf", component: PdfRoute }),
  createRoute({ getParentRoute: () => rootRoute, path: "/media", component: MediaRoute }),
  createRoute({ getParentRoute: () => rootRoute, path: "/queue", component: QueueRoute }),
  createRoute({ getParentRoute: () => rootRoute, path: "/diagnostics", component: HomeRoute }),
  createRoute({ getParentRoute: () => rootRoute, path: "/about", component: AboutRoute }),
]);

/**
 * Hash history: the production build is served from a custom Tauri protocol, so
 * path-based history would need the shell to rewrite unknown routes.
 */
export const router = createRouter({
  routeTree,
  history: createHashHistory(),
  defaultPreload: false,
});

declare module "@tanstack/react-router" {
  interface Register {
    router: typeof router;
  }
}
