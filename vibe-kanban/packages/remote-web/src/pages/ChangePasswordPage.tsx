import { useState } from "react";
import { useSearch } from "@tanstack/react-router";
import { changePassword } from "@remote/shared/lib/api";
import { getToken } from "@remote/shared/lib/auth/tokenManager";
import { BrandLogo } from "@remote/shared/components/BrandLogo";
import { Input } from "@vibe/ui/components/Input";
import { Label } from "@vibe/ui/components/Label";

const MIN_PASSWORD_LEN = 8;

export default function ChangePasswordPage() {
  // `from` matches the file-based route id; cast through unknown to satisfy
  // tanstack-router's known-route typing without depending on the generated tree.
  const search = useSearch({ strict: false }) as {
    must?: string;
    next?: string;
  };
  const mustChange = search.must === "1";
  const nextPath = search.next || "/";

  const [current, setCurrent] = useState("");
  const [next, setNext] = useState("");
  const [confirm, setConfirm] = useState("");
  const [pending, setPending] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [done, setDone] = useState(false);

  const handleSubmit = async () => {
    setError(null);
    if (next.length < MIN_PASSWORD_LEN) {
      setError(`New password must be at least ${MIN_PASSWORD_LEN} characters.`);
      return;
    }
    if (next === current) {
      setError("New password must be different from current password.");
      return;
    }
    if (next !== confirm) {
      setError("New passwords do not match.");
      return;
    }
    setPending(true);
    try {
      const token = await getToken();
      if (!token) {
        setError("Session expired. Please sign in again.");
        setPending(false);
        return;
      }
      await changePassword(current, next, token);
      setDone(true);
      // Brief pause so the success message renders before navigation.
      setTimeout(() => {
        window.location.replace(nextPath);
      }, 700);
    } catch (e) {
      setError(e instanceof Error ? e.message : "Failed to change password");
      setPending(false);
    }
  };

  return (
    <div className="h-screen overflow-auto bg-primary">
      <div className="mx-auto flex min-h-full w-full max-w-md flex-col justify-center px-base py-double">
        <div className="space-y-double rounded-sm border border-border bg-secondary p-double">
          <header className="space-y-double text-center">
            <div className="flex justify-center">
              <BrandLogo className="h-8 w-auto" />
            </div>
            <p className="text-sm text-low">
              {done ? "Password updated" : "Change your password"}
            </p>
          </header>

          {mustChange && !done && (
            <div className="rounded-sm border border-warning/30 bg-warning/10 p-base">
              <p className="text-sm text-high">
                Your account uses a temporary password. Please set a new one to
                continue.
              </p>
            </div>
          )}

          {error && (
            <div className="rounded-sm border border-error/30 bg-error/10 p-base">
              <p className="text-sm text-high">{error}</p>
            </div>
          )}

          {done ? (
            <div className="rounded-sm border border-border bg-primary p-base">
              <p className="text-sm text-normal">
                Password changed. Redirecting…
              </p>
            </div>
          ) : (
            <div className="space-y-3 rounded-sm border border-border bg-primary p-base">
              <div className="space-y-2">
                <Label htmlFor="cp-current">Current password</Label>
                <Input
                  id="cp-current"
                  type="password"
                  value={current}
                  onChange={(e) => setCurrent(e.target.value)}
                  autoComplete="current-password"
                  placeholder={
                    mustChange ? "Temporary password" : "Current password"
                  }
                />
              </div>
              <div className="space-y-2">
                <Label htmlFor="cp-new">New password</Label>
                <Input
                  id="cp-new"
                  type="password"
                  value={next}
                  onChange={(e) => setNext(e.target.value)}
                  autoComplete="new-password"
                  placeholder={`At least ${MIN_PASSWORD_LEN} characters`}
                />
              </div>
              <div className="space-y-2">
                <Label htmlFor="cp-confirm">Confirm new password</Label>
                <Input
                  id="cp-confirm"
                  type="password"
                  value={confirm}
                  onChange={(e) => setConfirm(e.target.value)}
                  autoComplete="new-password"
                />
              </div>
              <button
                type="button"
                className="w-full rounded-sm bg-brand px-base py-half text-sm font-medium text-on-brand transition-colors hover:bg-brand-hover disabled:cursor-not-allowed disabled:opacity-50"
                onClick={() => void handleSubmit()}
                disabled={pending || !current || !next || !confirm}
              >
                {pending ? "Saving…" : "Save new password"}
              </button>
            </div>
          )}
        </div>
      </div>
    </div>
  );
}
