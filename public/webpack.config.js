import path from 'path';

export default {
    entry: './src/index.js',
    // module: {
    //     rules: [
    //         {
    //             test: /\.ts?$/,
    //             use: 'ts-loader',
    //             exclude: /node_modules/,
    //         }
    //     ]
    // },
    // resolve: {
    //     extensions: ['.ts', '.js']
    // },
    output: {
        filename: 'bundle.js',
        path: path.resolve(import.meta.dirname, 'dist')
    }
};