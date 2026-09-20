# Refactor workflow

1. Define the structural problem, desired quality improvement, preserved observable behavior, and scope boundary.
2. Establish characterization evidence for behavior that could regress. Complete when every affected public contract is protected or explicitly assessed as low risk.
3. Make small reversible transformations with coherent intermediate states. Keep semantic changes separate.
4. Re-run focused tests after each meaningful transformation and the broader affected checks at the end.
5. Compare before and after against the targeted Core Rules. A refactor is complete only when the named quality improved without widening the public contract or changing behavior.
6. Report evidence, any intentional interface movement, and deferred cleanup.
