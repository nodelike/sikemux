import 'expo-router/entry';
import { AppRegistry, Platform } from 'react-native';

import { answerTask } from './src/notify/cards';

if (Platform.OS === 'android') AppRegistry.registerHeadlessTask('SikemuxAnswer', () => answerTask);
