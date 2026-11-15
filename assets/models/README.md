# Squad-Air Optional DeepFilterNet3 ONNX Model

To enable the **DeepFilterNet3** neural noise suppressor option in Settings:
1. Place the exported ONNX model at `./assets/models/deepfilternet3.onnx` (or inside your OS config folder `squad-air/models/deepfilternet3.onnx`).
2. If this file is not present, `squad-air` automatically falls back to the built-in **RNNoise (`nnnoiseless`)** model with zero startup errors.