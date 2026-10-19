Models based on Transformers \[91\], such as BERT \[22, 63\], are wildly successful for a wide variety of Natural Language Processing (NLP) tasks and consequently are mainstay of modern NLP research. Their versatility and robustness are the primary drivers behind the wide-scale adoption of Transformers. The model is easily adapted for a diverse range of sequence based tasks – as a seq2seq model for translation \[91\], summarization \[66\], generation \[15\], etc. or as a standalone encoders for sentiment analysis \[83\], POS tagging \[65\], machine reading comprehension \[93\], etc. – and it is known to vastly outperform previous sequence models like LSTM \[37\]. The key innovation in Transformers is the introduction of a self-attention mechanism, which can be evaluated in parallel for each token of the input sequence, eliminating the sequential dependency in recurrent neural networks, like LSTM. This parallelism enables Transformers to leverage the full power of modern SIMD hardware accelerators like GPUs/TPUs, thereby facilitating training of NLP models on datasets of unprecedented size. This ability to train on large scale data has led to surfacing of models like BERT \[22\] and T5 \[75\], which pretrain transformers on large general purpose corpora and transfer the knowledge to downstream task. The pretraining has led to significant improvement in low data regime downstream tasks \[51\] as well as tasks with sufficient data \[101\] and thus have been a major force behind the ubiquity of transformers in contemporary NLP.

The self-attention mechanism overcomes constraints of RNNs (namely the sequential nature of RNN) by allowing each token in the input sequence to attend independently to every other token in the sequence. This design choice has several interesting repercussions. In particular, the full self-attention have computational and memory requirement that is quadratic in the sequence length. We note that while the corpus can be large, the sequence length, which provides the context in many applications is very limited. Using commonly available current hardware and model sizes, this requirement

---

translates to roughly being able to handle input sequences of length 512 tokens. This reduces its direct applicability to tasks that require larger context, like QA \[60\], document classification, etc.

However, while we know that self-attention and Transformers are useful, our theoretical understanding is rudimentary. What aspects of the self-attention model are necessary for its performance? What can we say about the expressivity of Transformers and similar models? Apriori, it was not even clear from the design if the proposed self-attention mechanism was as effective as RNNs. For example, the self-attention does not even obey sequence order as it is permutation equivariant. This concern has been partially resolved, as Yun et al. \[104\] showed that transformers are expressive enough to capture all continuous sequence to sequence functions with a compact domain. Meanwhile, Pérez et al. \[72\] showed that the full transformer is Turing Complete (i.e. can simulate a full Turing machine). Two natural questions arise: Can we achieve the empirical benefits of a fully quadratic self-attention scheme using fewer inner-products? Do these sparse attention mechanisms preserve the expressivity and flexibility of the original network?

In this paper, we address both the above questions and produce a sparse attention mechanism that improves performance on a multitude of tasks that require long contexts. We systematically develop BigBird, an attention mechanism whose complexity is linear in the number of tokens (Sec. 2). We take inspiration from graph sparsification methods and understand where the proof for expressiveness of Transformers breaks down when full-attention is relaxed to form the proposed attention pattern. This understanding helped us develop BigBird, which is theoretically as expressive and also empirically useful. In particular, our BigBird consists of three main part:

• A set of g global tokens attending on all parts of the sequence.

• All tokens attending to a set of \\(w\\) local neighboring tokens.

• All tokens attending to a set of r random tokens.

This leads to a high performing attention mechanism scaling to much longer sequence lengths (8x).

To summarize, our main contributions are:

1\. BIGBIRD satisfies all the known theoretical properties of full transformer (Sec. 3). In particular, we show that adding extra tokens allows one to express all continuous sequence to sequence functions with only \\(O(n)\\)-inner products. Furthermore, we show that under standard assumptions regarding precision, BIGBIRD is Turing complete.

2\. Empirically, we show that the extended context modelled by BIGBIRD benefits variety of NLP tasks. We achieve state of the art results for question answering and document summarization on a number of different datasets. Summary of these results are presented in Sec. 4.

3\. Lastly, we introduce a novel application of attention based models where long contexts are beneficial: extracting contextual representations of genomics sequences like DNA. With longer masked LM pretraining, BiGBIRD improves performance on downstream tasks such as promoter- region and chromatin profile prediction (Sec. 5).
