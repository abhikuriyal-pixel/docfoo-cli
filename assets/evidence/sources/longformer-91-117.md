**Sliding Window** Given the importance of local context (Kovaleva et al., 2019), our attention pat- tern employs a fixed-size window attention sur- rounding each token. Using multiple stacked lay- ers of such windowed attention results in a large receptive field, where top layers have access to all input locations and have the capacity to build repre- sentations that incorporate information across the entire input, similar to CNNs (Wu et al., 2019). Given a fixed window size \\(w\\), each token attends to \\(\\frac\{1\}\{2\}w\\) tokens on each side (Fig. 2b). The com- putation complexity of this pattern is \\(O(n \\times w)\\),

²SQuAD contexts typically fit within the 512 limit, and MRQA is constructed by dropping long-document examples.

³All were published on arXiv after Longformer.

---

which scales linearly with input sequence length \\(n\\). In a transformer with \\(\\ell\\) layers, the receptive field size at the top layer is \\(\\ell \\times w\\) (assuming \\(w\\) is fixed for all layers). Depending on the application, it might be helpful to use different values of \\(w\\) for each layer to balance between efficiency and model representation capacity (§4.1).

Dilated Sliding Window To further increase the receptive field without increasing computation, the sliding window can be “dilated”. This is analogous to dilated CNNs (van den Oord et al., 2016) where the window has gaps of size dilation \\(d\\) (Fig. 2c). Assuming a fixed \\(d\\) and \\(w\\) for all layers, the recep- tive field is \\(\\ell \\times d \\times w\\), which can reach tens of thousands of tokens even for small values of \\(d\\).

In multi-headed attention, each attention head computes a different attention score. We found set- tings with different dilation configurations per head improves performance by allowing some heads without dilation to focus on local context, while others with dilation focus on longer context.

Global Attention In state-of-the-art BERT-style models for natural language tasks, the optimal in- put representation differs from language modeling and varies by task. For masked language modeling (MLM), the model uses local context to predict the masked word, while for classification, the model ag- gregates the representation of the whole sequence into a special token (\[CLS\] in case of BERT). For QA, the question and document are concatenated, allowing the model to compare the question with the document through self-attention.

In our case, the windowed and dilated attention are not flexible enough to learn task-specific repre- sentations. Accordingly, we add “global attention” on few pre-selected input locations. Importantly, we make this attention operation symmetric: that is, a token with a global attention attends to all tokens across the sequence, and all tokens in the sequence attend to it. Fig. 2d shows an example of a sliding window attention with global attention at a few tokens at custom locations. For example for classification, global attention is used for the \[CLS\] token while in QA global attention is pro- vided on all question tokens. Since the number of such tokens is small relative to and independent of \\(n\\) the complexity of the combined local and global attention is still \\(O(n)\\). While specifying global attention is task specific, it is a easy way to add in- ductive bias to the model’s attention, and it is much

simpler than existing task specific approaches that use complex architecture to combine information across smaller input chunks.

Linear Projections for Global Attention Re- call that given the linear projections \\(Q, K, V\\), the Transformer model (Vaswani et al., 2017) computes attention scores as follows:

$$
Attention(Q, K, V) = softmax \(\left(\frac{QK^T}{\sqrt{d_k}}\right)V\)\tag{1}
$$

We use two sets of projections, \\(Q\_s, K\_s, V\_s\\) to compute attention scores of sliding window attention, and \\(Q\_g, K\_g, V\_g\\) to compute attention scores for the global attention. The additional projections provide flexibility to model the different types of attention, which we show is critical for best performance on downstream tasks. \\(Q\_g, K\_g, V\_g\\) are all initialized with values that match \\(Q\_s, K\_s, V\_s\\).
