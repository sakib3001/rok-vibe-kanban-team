-- Protect the service account's team-org admin membership.
--
-- The service account (admin@rokomari.io) powers the ingestion API and the
-- invite tooling. If it loses admin membership of "Rokomari SE Team", both
-- break. This script (idempotent) ensures the membership exists and installs a
-- trigger that blocks DELETE and any role-downgrade of that specific row —
-- via the UI, the API, or manual SQL alike.
--
-- Apply:
--   docker compose exec -T postgres psql -U remote -d remote < sql/protect-service-account.sql
--
-- (A Postgres superuser could still disable/drop the trigger; this guards the
--  application + normal operations, which is where the risk is.)

-- 1) Ensure the membership exists (self-heal if it was removed).
INSERT INTO organization_member_metadata (organization_id, user_id, role)
SELECT o.id, u.id, 'admin'
FROM organizations o, users u
WHERE o.name = 'Rokomari SE Team' AND o.is_personal = false
  AND lower(u.email) = 'admin@rokomari.io'
ON CONFLICT (organization_id, user_id) DO UPDATE SET role = 'admin';

-- 2) Guard function: block remove/demote of the protected membership.
CREATE OR REPLACE FUNCTION protect_service_account_membership()
RETURNS trigger AS $$
DECLARE
  svc_user uuid;
  team_org uuid;
BEGIN
  SELECT id INTO svc_user FROM users
    WHERE lower(email) = 'admin@rokomari.io';
  SELECT id INTO team_org FROM organizations
    WHERE name = 'Rokomari SE Team' AND is_personal = false;

  IF svc_user IS NOT NULL AND team_org IS NOT NULL
     AND OLD.user_id = svc_user AND OLD.organization_id = team_org THEN
    IF TG_OP = 'DELETE' THEN
      RAISE EXCEPTION
        'Protected: service account (admin@rokomari.io) cannot be removed from "Rokomari SE Team".';
    ELSIF TG_OP = 'UPDATE' AND NEW.role <> 'admin' THEN
      RAISE EXCEPTION
        'Protected: service account (admin@rokomari.io) must remain an admin of "Rokomari SE Team".';
    END IF;
  END IF;
  RETURN CASE WHEN TG_OP = 'DELETE' THEN OLD ELSE NEW END;
END;
$$ LANGUAGE plpgsql;

-- 3) Install the trigger (idempotent).
DROP TRIGGER IF EXISTS protect_service_account ON organization_member_metadata;
CREATE TRIGGER protect_service_account
  BEFORE DELETE OR UPDATE ON organization_member_metadata
  FOR EACH ROW EXECUTE FUNCTION protect_service_account_membership();
