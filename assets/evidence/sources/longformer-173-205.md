Current state-of-the-art systems for many NLP tasks finetune a pretrained model with task super- vision (e.g. BERT). One of our main motivations is to develop such a model suitable for long docu- ment tasks. To do so, we pretrained Longformer on a document corpus and finetune it for six tasks, including classification, QA and coreference resolu- tion. The resulting model can process sequences up to 4,096 tokens long (8 times longer than BERT)⁵.

We pretrain Longformer with masked language modeling (MLM), where the goal is to recover randomly masked tokens in a sequence. Since MLM pretraining is expensive, we continue pre- training from the RoBERTa (Liu et al., 2019) re- leased checkpoint, while only making the minimal

![A three-column table with headers “Model,” “base,” and “large.” Thin horizontal rules appear above the header, below it, and beneath the final row. The “Model” column is left-aligned; the two numeric columns are right-aligned. Rows, in order: - RoBERTa (seqlen: 512) — base: 1.846; large: 1.496 - Longformer (seqlen: 4,096) — base: 10.299; large: 8.738 - + copy position embeddings — base: 1.957; large: 1.597 - + 2K gradient updates — base: 1.753; large: 1.414 - + 65K gradient updates — base: 1.705; large: 1.358 - Longformer (train extra pos. embed. only) — base: 1.850; large: 1.504 The three rows beginning with “+” are indented beneath the Longformer row.](assets/table_5_p06_05.jpg)

Table 5: MLM BPC for RoBERTa and various pre-trained Longformer configurations.

changes necessary to support Longformer’s attention mechanism. Note that our attention pattern can be plugged into any pretrained transformer model without the need to change the model architecture.

Attention Pattern We use sliding window attention with window size of 512, therefore using the same amount of computation as RoBERTa.⁶

Position Embeddings RoBERTa uses learned absolute position embeddings with the maximum position being 512. To support longer documents, we add extra position embeddings to support up to position 4,096. To leverage RoBERTa’s pretrained weights, instead of randomly initializing the new position embeddings, we initialize them by copying the 512 position embeddings from RoBERTa multiple times as analysis of BERT’s attention heads shows a strong learned bias to attending to local context, including the previous or next token (Clark et al., 2019). Using the copy initialization preserves this local structure everywhere except at the partition boundaries. Despite its simplicity, we found this to be a very effective (see Tab. 5), allowing Longformer pretraining to rapidly converge with a small number of gradient updates.

**Continued MLM Pretraining** We pretrain Longformer using fairseq (Ott et al., 2019) on a corpus of long documents that we compiled (see Appendix C for corpus details). We train two model sizes, a base model and a large model. Both models are trained for 65K gradient updates with sequences length 4,096, batch size 64 (2^\{18\} tokens), maximum learning rate of 3e-5, linear warmup of 500 steps, followed by a power 3 polynomial decay. The rest of the hyperparameters are the same as RoBERTa.

Tab. 5 shows the BPC on the development set of our training corpus. The first row shows a 1.846

⁴One caveat is that the ordering of end performance will not agree with that at step 150K. However, this approximation saves the huge cost of running every experiment to completion.

⁶Adding dilation on a few heads as in §4.1 hurt performance, likely because it is not compatible with the pretrained RoBERTa weights. Retraining such model from scratch might be needed to improve performance.

⁵Sequences up to 16K are possible on current GPUs.

---

![A two-row table with seven columns. The headers are “Wordpieces,” “WH,” “TQA,” “HQA,” “ON,” “IMDB,” and “HY.” The “avg.” row contains 1,535; 6,589; 1,316; 506; 300; and 705 under WH through HY, respectively. The “95th pctl.” row contains 3,627; 17,126; 1,889; 1,147; 705; and 1,975. Thin horizontal rules appear above and below the table and beneath the header row.](assets/table_6_p07_06.jpg)

Table 6: Average and 95th percentile of context length of datasets in wordpieces. WH: WikiHop, TQA: Triv- iaQA, HQA: HotpotQA, ON: OntoNotes, HY: Hyper- partisan news

BPC using RoBERTa-base, which is comparable to the 1.880 BPC reported on the RoBERTa paper on their corpus. This indicates our training corpus is from a distribution close to that used to train RoBERTa. The following two rows show the per- formance of Longformer before pretraining with randomly initialized position embeddings and with copied position embeddings. The significant differ- ence indicates the importance of the copy initial- ization, and the relative small difference between the RoBERTa BPC and the initialized BPC indi- cates that our sliding window attention is working well with the RoBERTa weights. The following two rows show the impact of continuing pretrain- ing. Training for 2K steps improves BPC from 1.957 to 1.753, which further decreases to 1.705 af- ter 65K steps, demonstrating the model is learning to better utilize the sliding window attention and longer context. Similar patterns are observed with RoBERTa-large and Longformer-large.

Frozen RoBERTa Weights We also pretrained Longformer while freezing all RoBERTa weights, and only training the new position embeddings. The motivation for this configuration is to perfectly preserve the RoBERTa performance on short doc- uments. This configuration has a BPC of 1.850 (down from 1.957 at initialization), but higher than 1.705 where all the weights are trainable.
