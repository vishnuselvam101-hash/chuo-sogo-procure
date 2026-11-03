import { lazy, Suspense, type ReactNode } from "react";
import { useTranslation } from "react-i18next";

// Loaded on its own so the shader code never sits in the main bundle.
const AmbientScene = lazy(() => import("./AmbientScene"));

/** The sign-in screens: the ambient scene behind, the form in front. */
export function LoginStage({ children }: { children: ReactNode }) {
  const { t } = useTranslation();
  return (
    <main className="login">
      <Suspense fallback={null}>
        <AmbientScene />
      </Suspense>
      <div className="login-inner">
        {children}
        <p className="login-tagline">{t("app.tagline")}</p>
      </div>
    </main>
  );
}
