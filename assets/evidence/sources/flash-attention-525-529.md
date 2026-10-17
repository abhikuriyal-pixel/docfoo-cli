We first derive the forward and backward passes of attention and show that they can be computed in a memory-efficient manner (requiring extra memory linear instead of quadratic in the sequence length). Though they reduce the amount of extra memory required, naively they still incur quadratic HBM accesses, resulting in slower execution speed. We describe the FLASHATTENTION algorithm to implement both the forward

---

and the backward passes on GPUs that reduces HBM accesses, leading to both faster runtime and smaller memory footprint.
