import { Link } from 'react-router-dom';
import { CircleQuestionMark } from 'lucide-react';
import { Button, EmptyState } from '../components';

export default function NotFound() {
	return (
		<div className="page container">
			<EmptyState
				icon={CircleQuestionMark}
				title="页面不存在"
				action={
					<Link to="/library">
						<Button variant="primary">回书库</Button>
					</Link>
				}
			/>
		</div>
	);
}
