import { useEffect, useRef, type ReactNode } from "react";

/*
 * Dashboard header with a little depth: three decorative planes (a layout grid and two glass
 * tiles) that lean toward the pointer. Pure CSS 3D driven by two custom properties, updated at
 * most once per frame; off under prefers-reduced-motion. The text sits on a flat layer above.
 */
export function DashboardHero({ children }: { children: ReactNode }) {
  const ref = useRef<HTMLDivElement>(null);

  useEffect(() => {
    const el = ref.current;
    if (!el || typeof window.matchMedia !== "function") return;
    if (window.matchMedia("(prefers-reduced-motion: reduce)").matches) return;
    let raf = 0;
    const onMove = (e: PointerEvent) => {
      const r = el.getBoundingClientRect();
      const x = (e.clientX - r.left) / r.width - 0.5;
      const y = (e.clientY - r.top) / r.height - 0.5;
      cancelAnimationFrame(raf);
      raf = requestAnimationFrame(() => {
        el.style.setProperty("--tilt-x", x.toFixed(3));
        el.style.setProperty("--tilt-y", y.toFixed(3));
      });
    };
    const onLeave = () => {
      cancelAnimationFrame(raf);
      el.style.setProperty("--tilt-x", "0");
      el.style.setProperty("--tilt-y", "0");
    };
    el.addEventListener("pointermove", onMove);
    el.addEventListener("pointerleave", onLeave);
    return () => {
      cancelAnimationFrame(raf);
      el.removeEventListener("pointermove", onMove);
      el.removeEventListener("pointerleave", onLeave);
    };
  }, []);

  return (
    <header className="hero" ref={ref}>
      <div className="hero-scene" aria-hidden="true">
        <div className="hero-grid" />
        <div className="hero-tile hero-tile-a" />
        <div className="hero-tile hero-tile-b" />
        <div className="hero-tile hero-tile-c" />
      </div>
      <div className="hero-text">{children}</div>
    </header>
  );
}
