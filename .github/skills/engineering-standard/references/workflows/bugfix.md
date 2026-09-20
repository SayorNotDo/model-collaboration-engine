# Bugfix workflow

1. State expected behavior, observed behavior, and the smallest known reproduction. Complete when the failure is observable or the missing evidence is explicitly identified.
2. Trace from symptom to violated invariant or boundary; distinguish cause from downstream damage.
3. Add or identify a regression test that fails for the cause, not merely the symptom.
4. Apply the narrowest fix at the responsibility that owns the violated rule. Avoid broad cleanup unless necessary for a safe fix.
5. Run the regression test, affected suite, and applicable profile checks. Check adjacent boundary cases suggested by the cause.
6. Review applicable Core Rules and report the cause, fix evidence, exceptions, and remaining uncertainty.
