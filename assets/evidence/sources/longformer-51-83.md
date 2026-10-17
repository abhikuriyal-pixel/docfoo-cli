Long-Document Transformers Tab. 1 summa- rizes recent prior work on long documents. Two types of self-attention approaches have been ex- plored. The first is a left-to-right (ltr) approach that processes the document in chunks moving from left-to-right. While such models have been success- ful in autoregressive language modeling, they are unsuitable for transfer learning approaches with tasks that benefit from bidirectional context.

Our work falls within the other general approach that defines some form of sparse attention pattern and avoids computing the full quadratic attention matrix multiplication. The model with the most similar attention pattern to ours is Sparse Trans- former (Child et al., 2019), which uses a form of dilated sliding window of blocks of size 8x8 pro- vided by BlockSparse (Gray et al., 2017). Our implementation (§3) also includes a custom CUDA kernel, but it is more flexible and maintainable than BlockSparse which is implemented in C++, and designed for a specific version of TensorFlow. We also introduce additional task motivated global at- tention patterns suitable for common NLP tasks (§3) and show they are essential for good perfor- mance in the transfer learning setting.

A few models tried tasks other than autoregressive language modeling, which is a step forward because arguably focusing on language modeling as the primary evaluation has led to the development of models with limited applicability. BP-Transformer (Ye et al., 2019) evaluated on machine

---

![A square grid on a white background, outlined by a thick black border. The grid contains many small square cells, with black grid lines and a teal-green fill. A darker green diagonal band runs from the upper-left corner to the lower-right corner, with a subtle pixelated or stepped appearance. No axes, labels, legend, or text are visible.](assets/figure_3_p03_03.jpg)

(a) Full \\(n^2\\) attention

![A square grid with 20 columns and 20 rows, outlined by a thick black border. Thin black lines divide the cells. A diagonal band of green squares runs from the upper-left corner to the lower-right corner: a darker green line follows the main diagonal, with lighter green squares forming a band on either side. No labels, axes, legend, or text are visible.](assets/figure_4_p03_04.jpg)

(b) Sliding window attention

![A square grid of small cells, enclosed by a thick black border. A diagonal band of colored cells runs from the upper left toward the lower right, widening slightly across the middle. Most marked cells are pale mint green, with darker green cells interspersed in a repeating, staggered pattern; several darker cells cluster along the band’s center. The cells outside the band are unfilled white. No labels, axes, legend, or text are visible.](assets/figure_5_p03_05.jpg)

(c) Dilated sliding window

![A square grid diagram with a dark outer border and many thin black grid lines. The grid has pale mint-green shading in several bands: a thick diagonal stripe runs from the upper-left toward the lower-right; horizontal bands cross near the top, middle, and bottom; and vertical bands appear near the left edge and in the right half. The bands intersect to form darker green blocks. No text, axes, legend, or numeric labels are visible.](assets/figure_6_p03_06.jpg)

(d) Global+sliding window

Figure 2: Comparing the full self-attention pattern and the configuration of attention patterns in our Longformer.

translation (MT), but didn’t explore the pretrain- finetune setting. Blockwise attention (Qiu et al., 2019) pretrained their models and evaluated on question answering (QA). However, the evaluation is limited as it doesn’t include language modeling, and the QA datasets are of relatively short docu- ments,² therefore the effectiveness of this model on long document tasks remains unexplored.

Task-specific Models for Long Documents Many task-specific approaches have been developed to workaround the 512 limit of pretrained transformer models like BERT. The simplest approach just truncates the document, commonly used for classification (Xie et al., 2019). Another approach chunks the document into chunks of length 512 (could be overlapping), processes each chunk separately, then combines the activations with a task specific model (Joshi et al., 2019). A third approach popular for multihop and open domain QA tasks uses a two-stage model where the first stage retrieves relevant documents that are passed onto the second stage for answer extraction (Clark and Gardner, 2017; Chen et al., 2017). All of these approaches suffer from information loss due to truncation or cascading errors from the two stage approach. In contrast, Longformer can process long sequences without truncating or chunking, allowing us to adopt a much simpler approach that concatenates the available context and processes it in a single pass.

A few contemporaneous works³ have explored similar ideas to Longformer using local + global attention in Transformers, and pre-training it for long document natural language tasks. In particu- lar, ETC (Ainslie et al., 2020) uses a similar local + global attention instead of full self-attention to scale Transformers to long documents. Different from Longformer, ETC uses relative position em-

beddings (which we only used for the Autoregres- sive LM setting), introduces an additional training objective (CPC loss) for pre-training, and config- ures global attention in a slightly different way. It shows strong results on several tasks including reading comprehension and classification. GMAT (Gupta and Berant, 2020) uses a similar idea of few global locations in the input serving as global memory. BigBird (Zaheer et al., 2020) is an exten- sion over ETC with evaluation on additional tasks, including summarization. Importantly, through the- oretical analysis, BigBird shows that sparse Trans- formers are universal approximators of sequence functions and preserve these properties of the full self-attention.
